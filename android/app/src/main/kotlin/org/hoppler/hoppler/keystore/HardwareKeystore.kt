package org.hoppler.hoppler.keystore

import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyInfo
import android.security.keystore.KeyProperties
import android.security.keystore.StrongBoxUnavailableException
import android.util.Log
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.SecretKeyFactory
import javax.crypto.spec.GCMParameterSpec

/**
 * The wrapping key R0-F1 means by "master secret in platform hardware".
 *
 * An AES-256-GCM key generated inside the Android Keystore. The key material
 * never enters this process — `SecretKey` here is a handle, and every
 * encryption happens in the keystore daemon (or, with StrongBox, in a separate
 * security chip). It cannot be exported, and it cannot be used on any other
 * device, which is exactly the boundary the Rust side describes: storage lifted
 * off this phone is ciphertext nobody can open.
 *
 * The core calls in over JNI. See [install] for why the call goes that way.
 */
class HardwareKeystore {

    companion object {
        private const val TAG = "HopplerKeystore"
        private const val PROVIDER = "AndroidKeyStore"

        /**
         * Versioned, because rotating the wrapping key means a new alias and a
         * migration, not a silent replacement — replacing the key under this
         * name would leave every sealed secret unopenable with no way back.
         */
        private const val ALIAS = "hoppler/wrap/v1"

        /** GCM's standard nonce length, and what the platform's Cipher produces. */
        private const val IV_LEN = 12
        private const val TAG_BITS = 128

        /**
         * Hand the core a way to reach this object.
         *
         * Registration is pushed from here rather than pulled from Rust because
         * of how JNI resolves classes: a native thread's `FindClass` uses the
         * system class loader, which cannot see classes from the APK, so Rust
         * asking for `HardwareKeystore` by name would fail on every thread but
         * the one that entered from Java. Being handed an instance sidesteps it
         * — the core takes a global reference and reads the class off the
         * object itself, which works from any thread.
         *
         * Called from `MainActivity.configureFlutterEngine`, which runs before
         * the Dart entrypoint and therefore before anything asks the core to
         * open a store.
         */
        @JvmStatic
        fun install() {
            // Loaded here rather than relying on Dart having done it: this runs
            // before the Dart entrypoint, so on this path we are the first to
            // need the library. It is idempotent — Dart's later `DynamicLibrary`
            // open of the same soname finds it already mapped.
            System.loadLibrary("rust_lib_hoppler")
            HardwareKeystore().nativeInstall()
        }
    }

    /**
     * Hand `this` to the core, which keeps a global reference to it.
     *
     * An instance method with no arguments, deliberately. An `external` function
     * declared inside the companion object binds to the symbol
     * `..._HardwareKeystore_00024Companion_nativeInstall` — the mangled
     * `$Companion` — and the mismatch would not appear until the call, at
     * runtime, on a device. Here the symbol is exactly
     * `Java_org_hoppler_hoppler_keystore_HardwareKeystore_nativeInstall` and
     * the object the core needs is simply the receiver.
     */
    private external fun nativeInstall()

    /**
     * Encrypt `secret` so only this device can read it back.
     *
     * Returns `[ivLen][iv][ciphertext||tag]`, self-describing so the length is
     * never assumed on the way back in. Returns null on failure — the JNI
     * caller reads null as "the backend refused", which the Rust side turns
     * into an error rather than a missing secret.
     *
     * `label` is authenticated but not encrypted, which is what stops a blob
     * sealed for one label from opening under another.
     *
     * **`secret` is zeroed before this returns, on every path.** It is the copy
     * JNI made crossing in, and the caller is the core, which keeps its own and
     * zeroes that itself — so nothing observes the change. Said here anyway,
     * because a public method that empties its argument is not what anyone
     * reading the signature would assume.
     */
    fun wrap(label: String, secret: ByteArray): ByteArray? =
        try {
            val cipher = Cipher.getInstance("AES/GCM/NoPadding")
            cipher.init(Cipher.ENCRYPT_MODE, key())
            cipher.updateAAD(label.toByteArray(Charsets.UTF_8))
            val iv = cipher.iv
            require(iv.size == IV_LEN) { "unexpected GCM nonce length ${iv.size}" }
            val body = cipher.doFinal(secret)
            ByteArray(1 + iv.size + body.size).also {
                it[0] = iv.size.toByte()
                iv.copyInto(it, 1)
                body.copyInto(it, 1 + iv.size)
            }
        } catch (e: Exception) {
            // Message only — a stack trace here is fine but the secret is not,
            // and `e` never contains it.
            Log.w(TAG, "could not wrap: ${e.javaClass.simpleName}: ${e.message}")
            null
        } finally {
            // The core zeroes its own copy; this is the one JNI made crossing
            // in. It does not make the seed unreachable — the GC may have moved
            // it and left the original bytes behind — but leaving a live
            // reference full of key material for the lifetime of the heap would
            // be worse. Nobody who can read this heap is short of ways to read
            // the core's memory too, so this is hygiene, not a boundary.
            secret.fill(0)
        }

    /** Reverse [wrap] for the same label. Null on any failure. */
    fun unwrap(label: String, blob: ByteArray): ByteArray? =
        try {
            val ivLen = blob[0].toInt()
            require(ivLen == IV_LEN && blob.size > 1 + ivLen) { "malformed wrapped blob" }
            val cipher = Cipher.getInstance("AES/GCM/NoPadding")
            cipher.init(
                Cipher.DECRYPT_MODE,
                key(),
                GCMParameterSpec(TAG_BITS, blob, 1, ivLen),
            )
            cipher.updateAAD(label.toByteArray(Charsets.UTF_8))
            cipher.doFinal(blob, 1 + ivLen, blob.size - 1 - ivLen)
        } catch (e: Exception) {
            Log.w(TAG, "could not unwrap: ${e.javaClass.simpleName}: ${e.message}")
            null
        }

    /**
     * The wrapping key, generated on first use.
     *
     * Deliberately **not** `setUserAuthenticationRequired`. That would bind
     * every unseal to a screen unlock, and the store master is unsealed while
     * the app is starting — R0-F1 gives first launch thirty seconds, and a
     * device that cannot open its own database until somebody authenticates
     * again is not that. The cost is that the key is usable by anything running
     * as this app, which the Rust module doc already states as the boundary.
     *
     * It also means the key survives a lock screen change: permanent
     * invalidation on that applies to auth-bound keys, not this one. What does
     * lose it is the app's data being removed or restored somewhere else —
     * uninstall, factory reset, or a backup opened on a different phone, since
     * keystore keys do not travel. Those arrive as an unwrap failure, which is
     * why the Rust side is so careful that a failure is never read as "no
     * identity here".
     */
    private fun key(): SecretKey {
        val store = KeyStore.getInstance(PROVIDER).apply { load(null) }
        (store.getKey(ALIAS, null) as? SecretKey)?.let { return it.also { k -> report(k) } }
        // StrongBox first: a separate chip, so extracting the key means
        // attacking hardware rather than the main SoC. Not every device has one
        // — the Pixel 8 Pro does, plenty of others do not — and asking for it
        // where it is absent throws rather than degrades, so the fallback is
        // required, not optional.
        return try {
            generate(strongBox = true)
        } catch (e: StrongBoxUnavailableException) {
            Log.i(TAG, "no StrongBox on this device; using the TEE-backed keystore")
            generate(strongBox = false)
        }.also { report(it) }
    }

    /**
     * Say where the key actually ended up.
     *
     * Asking for StrongBox and getting it are different things, and so are
     * "generated in the Android Keystore" and "generated in hardware" — the
     * provider is allowed to answer with a software-backed key, and everything
     * above this line would work identically. R0-F1's claim is about hardware,
     * so the claim needs a witness, and this is the only one available: the
     * device saying which it gave us.
     *
     * A warning rather than a refusal when it is software. Refusing would mean
     * an app that will not start on a device whose keystore is unusual, to
     * protect a property that is already degraded rather than broken — the
     * secrets are still no worse off than under file permissions alone. Loud
     * enough that the acceptance run reads it, and `keystore-hardware-check.sh`
     * fails on it.
     *
     * Content-free: a security level, not key material.
     */
    private fun report(key: SecretKey) {
        val level = try {
            val info = SecretKeyFactory.getInstance(key.algorithm, PROVIDER)
                .getKeySpec(key, KeyInfo::class.java) as KeyInfo
            when (info.securityLevel) {
                KeyProperties.SECURITY_LEVEL_STRONGBOX -> "strongbox"
                KeyProperties.SECURITY_LEVEL_TRUSTED_ENVIRONMENT -> "tee"
                KeyProperties.SECURITY_LEVEL_SOFTWARE -> "software"
                else -> "unknown"
            }
        } catch (e: Exception) {
            Log.w(TAG, "could not read the key's security level: ${e.message}")
            "unreadable"
        }
        if (level == "software" || level == "unknown" || level == "unreadable") {
            Log.w(TAG, "wrapping key backing: $level — NOT hardware")
        } else {
            Log.i(TAG, "wrapping key backing: $level")
        }
    }

    private fun generate(strongBox: Boolean): SecretKey {
        val spec = KeyGenParameterSpec.Builder(
            ALIAS,
            KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT,
        )
            .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
            .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
            .setKeySize(256)
            // Randomised encryption is on by default for GCM and must stay on:
            // turning it off would let the caller supply the nonce, and a
            // repeated GCM nonce under one key is a total loss of confidentiality.
            .setIsStrongBoxBacked(strongBox)
            .build()
        return KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, PROVIDER)
            .apply { init(spec) }
            .generateKey()
    }
}
