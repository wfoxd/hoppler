//! The Android half of [`wrapped`](super::wrapped): a wrapping key held by the
//! Android Keystore, reached over JNI.
//!
//! There is no crypto here. Every call goes out to `HardwareKeystore.kt`, which
//! asks the keystore daemon to encrypt with a key this process cannot read.
//! What lives in this module is the plumbing: how the core finds that object,
//! and how a Java exception becomes a [`KeystoreError`].
//!
//! # Why registration is pushed from Kotlin
//!
//! JNI resolves classes with the class loader of whatever is on the call stack.
//! A thread the core created has nothing Java on its stack, so `FindClass`
//! falls back to the system loader — which knows the platform's classes and
//! nothing from the APK. Asking for `org/hoppler/hoppler/keystore/HardwareKeystore`
//! by name therefore works only on a thread that entered from Java, and the
//! core's threads did not.
//!
//! So Kotlin calls in with an instance instead, and [`install`] keeps a global
//! reference to it. Reading the class off a live object works from any thread,
//! and the reference keeps it alive across the GC.

use std::sync::OnceLock;

use jni::objects::{GlobalRef, JByteArray, JObject, JString, JValue};
use jni::{JNIEnv, JavaVM};
use zeroize::Zeroizing;

use super::keystore::KeystoreError;
use super::wrapped::Hardware;

/// The registered keystore object, and the VM needed to call it.
///
/// Set once, from [`install`]. `OnceLock` rather than a mutable global because
/// a second registration would mean two views of one key and no way to say
/// which is current; there is exactly one `HardwareKeystore` and it lives as
/// long as the process.
static REGISTERED: OnceLock<(JavaVM, GlobalRef)> = OnceLock::new();

/// Take the reference Kotlin is offering.
///
/// Called from `HardwareKeystore.install()` during `configureFlutterEngine`,
/// which is before the Dart entrypoint and so before anything opens a store.
///
/// # Safety
///
/// Called by the JVM with a valid environment and receiver. An instance method
/// with no arguments, so the second parameter is `this` — the object being
/// registered. Nothing here escapes past the global reference, which is what
/// `new_global_ref` exists to make safe to keep.
#[no_mangle]
pub extern "system" fn Java_org_hoppler_hoppler_keystore_HardwareKeystore_nativeInstall(
    env: JNIEnv,
    keystore: JObject,
) {
    let Ok(vm) = env.get_java_vm() else {
        log::error!("keystore registration: no JavaVM");
        return;
    };
    let Ok(global) = env.new_global_ref(keystore) else {
        log::error!("keystore registration: could not hold a reference");
        return;
    };
    // Ignored rather than asserted: a second call means the activity was
    // rebuilt, which is normal, and the first reference is as good as the
    // second. Loud only if it ever stops being the same object, which nothing
    // in the app arranges.
    if REGISTERED.set((vm, global)).is_err() {
        log::debug!("keystore already registered; keeping the first reference");
    }
}

/// Whether Kotlin has registered itself.
///
/// The engine asks before composing a [`WrappedKeystore`](super::wrapped::WrappedKeystore),
/// because the alternative to knowing is worse — see `platform_keystore`.
pub fn is_available() -> bool {
    REGISTERED.get().is_some()
}

/// The Android Keystore, as a [`Hardware`].
pub struct AndroidHardware;

impl AndroidHardware {
    /// Call one of the two Kotlin methods. They have the same shape, so the
    /// exception handling and the null convention are written once.
    ///
    /// A Java exception must be cleared before returning, or it stays pending
    /// on the thread and the *next* JNI call fails for a reason that has
    /// nothing to do with it — a class of bug that surfaces far from its cause.
    fn call(&self, method: &str, label: &str, bytes: &[u8]) -> Result<Vec<u8>, KeystoreError> {
        let (vm, keystore) = REGISTERED.get().ok_or_else(|| {
            log::error!("{method}: the hardware keystore was never registered");
            KeystoreError::Backend
        })?;
        let mut env = vm.attach_current_thread().map_err(|e| {
            log::error!("{method}: could not attach to the JVM: {e}");
            KeystoreError::Backend
        })?;

        let result = (|| -> Result<Option<Vec<u8>>, jni::errors::Error> {
            let label: JString = env.new_string(label)?;
            let bytes: JByteArray = env.byte_array_from_slice(bytes)?;
            let out = env.call_method(
                keystore.as_obj(),
                method,
                "(Ljava/lang/String;[B)[B",
                &[JValue::Object(&label), JValue::Object(&bytes)],
            )?;
            let out = out.l()?;
            if out.is_null() {
                return Ok(None);
            }
            Ok(Some(env.convert_byte_array(JByteArray::from(out))?))
        })();

        if env.exception_check().unwrap_or(false) {
            // Described, not propagated: the message can name a cipher or an
            // alias, neither of which is secret, but it is cleared either way
            // so the thread is left usable.
            let _ = env.exception_describe();
            let _ = env.exception_clear();
        }

        match result {
            Ok(Some(bytes)) => Ok(bytes),
            // Kotlin's null is a refusal it has already logged, so this does
            // not log again.
            Ok(None) => Err(KeystoreError::Backend),
            Err(e) => {
                log::error!("{method}: JNI call failed: {e}");
                Err(KeystoreError::Backend)
            }
        }
    }
}

impl Hardware for AndroidHardware {
    fn wrap(&self, label: &str, secret: &[u8]) -> Result<Vec<u8>, KeystoreError> {
        self.call("wrap", label, secret)
    }

    fn unwrap(&self, label: &str, wrapped: &[u8]) -> Result<Zeroizing<Vec<u8>>, KeystoreError> {
        self.call("unwrap", label, wrapped).map(Zeroizing::new)
    }
}
