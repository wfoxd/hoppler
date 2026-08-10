package org.hoppler.hoppler.ble

import android.annotation.SuppressLint
import android.annotation.TargetApi
import android.bluetooth.BluetoothAdapter
import android.bluetooth.BluetoothDevice
import android.bluetooth.BluetoothGatt
import android.bluetooth.BluetoothGattCallback
import android.bluetooth.BluetoothManager
import android.bluetooth.BluetoothProfile
import android.bluetooth.BluetoothServerSocket
import android.bluetooth.BluetoothSocket
import android.bluetooth.BluetoothSocketException
import android.bluetooth.le.AdvertiseData
import android.bluetooth.le.AdvertisingSet
import android.bluetooth.le.AdvertisingSetCallback
import android.bluetooth.le.AdvertisingSetParameters
import android.bluetooth.le.BluetoothLeAdvertiser
import android.bluetooth.le.BluetoothLeScanner
import android.bluetooth.le.ScanCallback
import android.bluetooth.le.ScanFilter
import android.bluetooth.le.ScanResult
import android.bluetooth.le.ScanSettings
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.PackageManager
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.os.ParcelUuid
import android.util.Log
import io.flutter.plugin.common.EventChannel
import io.flutter.plugin.common.MethodCall
import io.flutter.plugin.common.MethodChannel
import java.io.IOException
import java.util.UUID
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.CountDownLatch
import java.util.concurrent.Executors
import java.util.concurrent.RejectedExecutionException
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicReference

/**
 * The Android BLE adapter (T08b).
 *
 * Implements `docs/BLE_CHANNEL.md` v1 and nothing beyond it. Every rule the
 * core can enforce is enforced in Rust; what is left here is the short list in
 * §5 — a real radio stop, contiguous ordered writes, a `writeComplete` per
 * accepted send, and stable ids taken from the advertisement rather than the
 * MAC address.
 *
 * Deliberately reports facts rather than tidying them: duplicate opens,
 * closes for pipes that never opened, and bytes arriving after a hang-up are
 * all passed straight through, because the core has the context to resolve
 * them and is tested doing so.
 */
class BleAdapter(private val context: Context) : MethodChannel.MethodCallHandler,
    EventChannel.StreamHandler {

    companion object {
        /**
         * The adapter logs what the core cannot see.
         *
         * The Rust side is well instrumented and the radio layer was silent,
         * which meant a failed dial could be observed but never explained —
         * three hardware diagnoses in a row were guesses because of it. What is
         * logged here is chosen to answer the questions the core cannot: which
         * PSM we listen on, which we dial, whether an accept ever fires, and
         * what a failure actually threw.
         *
         * **No addresses.** A line pairing a peer's rotating id with its
         * Bluetooth address would outlive the rotation and undo R0-F2 — the
         * same leak a security review already found in the core's logging.
         * Correlate with `bt_stack` by timestamp if an address is ever needed.
         */
        private const val TAG = "hoppler-ble"

        const val METHOD_CHANNEL = "org.hoppler/ble"
        const val EVENT_CHANNEL = "org.hoppler/ble/events"
        const val CHANNEL_VERSION = 1

        /**
         * Hoppler's service UUID — a fixed random 128-bit value, not a
         * shortened Bluetooth SIG id. Scanners filter on it in hardware, which
         * is what keeps a scan from waking the CPU for every BLE device in the
         * room (R0-N4).
         */
        val SERVICE_UUID: UUID = UUID.fromString("6f8c1d2e-7a3b-4c5d-9e0f-1a2b3c4d5e6f")
        val SERVICE_PARCEL: ParcelUuid = ParcelUuid(SERVICE_UUID)

        /** A sighting older than this is reported as `peerLost`. */
        /**
         * Every event type this adapter emits, and the whole of its side of the
         * contract with `lib/src/ble/ble_channel.dart`.
         *
         * Named rather than written inline at each `emit`, because these
         * strings are one half of a protocol whose other half is in another
         * language. Nothing in either toolchain checks that the two agree: the
         * Dart decoder is tested against literals Dart writes itself, so
         * renaming one here would leave every test in both languages passing
         * and the feature silently dead on a device.
         *
         * `ble_channel_vocabulary_test.dart` reads this block and asserts the
         * two sets match. That is why the declarations below are plain
         * `const val NAME = "value"` lines — keep them that way, or the test
         * that couples the languages stops being able to see them.
         */
        const val EVENT_PEER_FOUND = "peerFound"
        const val EVENT_PEER_LOST = "peerLost"
        const val EVENT_PIPE_OPENED = "pipeOpened"
        const val EVENT_PIPE_FAILED = "pipeFailed"
        const val EVENT_PIPE_CLOSED = "pipeClosed"
        const val EVENT_RECEIVED = "received"
        const val EVENT_WRITE_COMPLETE = "writeComplete"
        const val EVENT_AVAILABILITY = "availability"

        const val PEER_TTL_MS = 15_000L
        private const val AGE_SWEEP_MS = 3_000L

        /**
         * How long to wait for the LE link a dial rides on.
         *
         * A peer advertising at `INTERVAL_MEDIUM` is connectable a few times a
         * second, so a link that has not come up in ten seconds is not slow,
         * it is not coming. Long enough to be a real answer, short enough that
         * a dial to a peer that has walked away still ends.
         */
        private const val LINK_TIMEOUT_MS = 10_000L

        /**
         * How many GATT connections count as "the phone is full".
         *
         * `GATT_MAX_PHY_CHANNEL` is commonly 7 and is not readable from an app,
         * so this is a threshold for phrasing a message, never a decision to
         * skip a dial — a device with a larger pool must still be allowed to
         * try. Chosen at the common value: at seven the observed failure was
         * total (§5.0.15).
         */
        private const val CROWDED_LINKS = 7

        /**
         * What to tell the person when the link would not come up.
         *
         * States the facts and stops. Android's GATT pool is shared with every
         * app and commonly holds seven, so a phone already near that many
         * cannot open another link however healthy its radio is (§5.0.15) — but
         * a refused link has other causes too, and naming exhaustion as *the*
         * cause would be the same confident guess that cost eight hardware
         * runs. The count is evidence; the reader draws the conclusion.
         *
         * A number, never an address. This one reaches the screen, and R0-F2
         * does not stop at the log.
         *
         * In the companion so it can be tested without a `Context` — the whole
         * class of bug this rung keeps producing is one no device test catches
         * and no reviewer should have to.
         */
        fun linkFailureReason(gattLinks: Int): String = when {
            // -1 is the adapter failing to count at all; inventing "0 open"
            // from that would be a number we do not have.
            gattLinks < 0 -> "Could not open a Bluetooth connection."
            // "around $CROWDED_LINKS", never "the limit". The real ceiling is
            // not readable from an app and differs by device, so stating it as
            // fact would be false on any phone with a larger pool — and at, say,
            // twenty open it would be absurd on its face. The measured count is
            // ours to assert; the ceiling is not.
            gattLinks >= CROWDED_LINKS ->
                "Could not open a Bluetooth connection. This phone already has " +
                    "$gattLinks open, and Android usually allows around " +
                    "$CROWDED_LINKS — turning Bluetooth off and on again frees them."
            else -> "Could not open a Bluetooth connection ($gattLinks already open)."
        }
    }

    private val main = Handler(Looper.getMainLooper())
    private val io = Executors.newCachedThreadPool()
    private var sink: EventChannel.EventSink? = null

    private val manager get() = context.getSystemService(Context.BLUETOOTH_SERVICE) as BluetoothManager
    private val adapter: BluetoothAdapter? get() = manager.adapter
    private val advertiser: BluetoothLeAdvertiser? get() = adapter?.bluetoothLeAdvertiser
    private val scanner: BluetoothLeScanner? get() = adapter?.bluetoothLeScanner

    private var advertiseCallback: AdvertisingSetCallback? = null
    private var scanCallback: ScanCallback? = null
    private var serverSocket: BluetoothServerSocket? = null
    private var localPsm: Int = 0
    private val shuttingDown = AtomicBoolean(false)

    /** Peers seen in advertisements: id → how to reach them. */
    private val seen = ConcurrentHashMap<String, Sighting>()

    /** Open pipes: id → socket and its serialised writer. */
    private val pipes = ConcurrentHashMap<String, Pipe>()

    /**
     * Peers with a dial in flight.
     *
     * `pipes` cannot serve: a dial that has not connected yet has no entry
     * there, so two `connect` calls landing in the same millisecond both pass
     * that check and both open an L2CAP channel to the same remote. Two
     * channels to one remote at once do not both succeed — on two phones,
     * neither did, and every Ping failed.
     *
     * The core has been fixed not to ask twice, but this stays: it is one line,
     * and the next thing to reach a peer twice — a retry, a double tap, a
     * second rung — would rediscover the same failure from scratch.
     */
    private val dialling = ConcurrentHashMap.newKeySet<String>()

    private class Sighting(
        val device: BluetoothDevice,
        val advert: BleFraming.Advert,
        var lastSeen: Long
    )

    /**
     * An open pipe, and the LE link underneath it if we raised one.
     *
     * [link] is null for a pipe we accepted — the dialer raised that ACL and
     * owns it. Ours is closed with the pipe, never separately, so there is one
     * place a client interface can leak from instead of several.
     */
    private class Pipe(val socket: BluetoothSocket, val link: BluetoothGatt? = null) {
        /** §5.2: one `send` is one contiguous, ordered write. */
        val writeLock = Any()

        fun release() {
            runCatching { socket.close() }
            link?.let { runCatching { it.close() } }
        }
    }

    // ── channel plumbing ────────────────────────────────────────────────────

    override fun onListen(arguments: Any?, events: EventChannel.EventSink?) {
        sink = events
        // Availability is the first thing the UI needs: an empty peer list with
        // Bluetooth off must not read as "nobody is nearby" (R0-F2).
        emitAvailability()
        val filter = IntentFilter(BluetoothAdapter.ACTION_STATE_CHANGED)
        // API 33+ refuses an unflagged registration outright.
        if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.TIRAMISU) {
            context.registerReceiver(bluetoothStateReceiver, filter, Context.RECEIVER_NOT_EXPORTED)
        } else {
            @Suppress("UnspecifiedRegisterReceiverFlag")
            context.registerReceiver(bluetoothStateReceiver, filter)
        }
        main.postDelayed(ageSweep, AGE_SWEEP_MS)
    }

    override fun onCancel(arguments: Any?) {
        runCatching { context.unregisterReceiver(bluetoothStateReceiver) }
        main.removeCallbacks(ageSweep)
        sink = null
    }

    /** Events may be raised from radio threads; the sink is main-thread only. */
    private fun emit(event: Map<String, Any?>) {
        main.post { sink?.success(event) }
    }

    private fun fail(result: MethodChannel.Result, code: String, message: String) {
        main.post { result.error(code, message, null) }
    }

    private fun ok(result: MethodChannel.Result, value: Any? = null) {
        main.post { result.success(value) }
    }

    /**
     * Hand work to the radio threads. False if the adapter is shutting down.
     *
     * `connect` and `send` answer their `MethodChannel.Result` *before* queuing,
     * because acceptance and outcome are separate (§5.4). After
     * `io.shutdownNow()` the queue rejects, and an escaping
     * `RejectedExecutionException` would reach `onMethodCall`'s catch, which
     * replies to that same result a second time — which Flutter treats as
     * fatal. So the shutdown path would take the app down with it.
     *
     * Dropping the work is not just the safe choice, it is the specified one:
     * §6 rule 6 says a shut-down adapter is inert, and an outcome event after
     * shutdown would be neither silent nor inert.
     */
    private fun submit(work: () -> Unit): Boolean {
        // Checked before the queue, not just caught after it. `shutdown` sets
        // this flag and only reaches `io.shutdownNow()` several statements
        // later, so between the two the pool still accepts — and work queued in
        // that window runs against half-torn-down state. Rejection alone would
        // never report it.
        if (shuttingDown.get()) return false
        return try {
            io.execute(work)
            true
        } catch (e: RejectedExecutionException) {
            false
        }
    }

    override fun onMethodCall(call: MethodCall, result: MethodChannel.Result) {
        try {
            when (call.method) {
                "version" -> ok(result, CHANNEL_VERSION)
                "setLocalId" -> {
                    localId = call.argument<String>("localId")!!
                    ok(result)
                }
                "startAdvertising" -> startAdvertising(
                    call.argument<ByteArray>("payload")!!,
                    result
                )
                "stopAdvertising" -> stopAdvertising(result)
                "startScanning" -> startScanning(result)
                "stopScanning" -> stopScanning(result)
                "connect" -> connect(call.argument<String>("peer")!!, result)
                "send" -> send(
                    call.argument<String>("peer")!!,
                    call.argument<ByteArray>("bytes")!!,
                    result
                )
                "disconnect" -> {
                    closePipe(call.argument<String>("peer")!!, report = true)
                    ok(result)
                }
                "shutdown" -> {
                    shutdown()
                    ok(result)
                }
                else -> main.post { result.notImplemented() }
            }
        } catch (e: SecurityException) {
            // Permission revoked while running. Reported, not crashed.
            fail(result, "unavailable", "permission denied: ${e.message}")
            emitAvailability("Hoppler needs permission to use Bluetooth")
        } catch (e: Exception) {
            fail(result, "io", e.message ?: e.toString())
        }
    }

    // ── advertising ─────────────────────────────────────────────────────────

    @SuppressLint("MissingPermission")
    private fun startAdvertising(payload: ByteArray, result: MethodChannel.Result) {
        val localId = this.localId
            ?: return fail(result, "unavailable", "setLocalId has not been called")
        val advertiser = advertiser
            ?: return fail(result, "unavailable", "Bluetooth is off or LE advertising unsupported")

        // The L2CAP listener must exist before we advertise its PSM.
        if (serverSocket == null && !openServerSocket()) {
            return fail(result, "unavailable", "could not open an L2CAP listener")
        }

        // Legacy advertising carries 31 bytes *total*, which the core's payload
        // budget alone exceeds. Extended advertising is not optional for us, so
        // a device without it is unavailable rather than silently truncated.
        val bt = adapter
        if (bt == null || !bt.isLeExtendedAdvertisingSupported) {
            return fail(
                result, "unavailable",
                "this device has no LE extended advertising; Hoppler needs it to carry a persona"
            )
        }
        val body = BleFraming.frame(localPsm, localId, payload)
        // 18 bytes of service-data AD overhead; see MAX_ADVERTISING_PAYLOAD.
        if (body.size + 18 > bt.leMaximumAdvertisingDataLength) {
            return fail(
                result, "payload_too_large",
                "advertisement is ${body.size + 18} bytes, radio takes ${bt.leMaximumAdvertisingDataLength}"
            )
        }

        stopAdvertisingInternal()
        val data = AdvertiseData.Builder()
            .addServiceUuid(SERVICE_PARCEL)
            .addServiceData(SERVICE_PARCEL, body)
            .setIncludeDeviceName(false) // a stable name would defeat rotation (R0-F2)
            .build()
        // Extended, connectable, non-scannable: BLE 5 forbids an extended
        // advertisement being both connectable and scannable, and connectable
        // is what the L2CAP dial needs.
        val params = AdvertisingSetParameters.Builder()
            .setLegacyMode(false)
            .setConnectable(true)
            .setScannable(false)
            .setInterval(AdvertisingSetParameters.INTERVAL_MEDIUM)
            .setTxPowerLevel(AdvertisingSetParameters.TX_POWER_MEDIUM)
            .build()

        val callback = object : AdvertisingSetCallback() {
            override fun onAdvertisingSetStarted(set: AdvertisingSet?, txPower: Int, status: Int) {
                if (status == ADVERTISE_SUCCESS) {
                    // The psm we publish, against the psm we opened above: if
                    // these ever disagree, every dial to us fails and neither
                    // side can tell that from being out of range.
                    Log.i(TAG, "advertising ${body.size} bytes carrying psm $localPsm")
                    ok(result)
                } else {
                    Log.w(TAG, "the radio refused to advertise (status $status)")
                    advertiseCallback = null
                    fail(result, "io", "advertising failed (status $status)")
                }
            }

            override fun onAdvertisingEnabled(set: AdvertisingSet?, enable: Boolean, status: Int) {
                // A connectable advertisement stops the moment a peer connects
                // — the controller does it, not us. If it never resumes we go
                // dark to everyone else, which from their side is
                // indistinguishable from having walked away. Logged rather
                // than assumed either way: it decides whether a mesh of three
                // works, and nothing else in the system reports it.
                if (!enable) Log.i(TAG, "the advertisement went quiet (status $status)")
            }
        }
        advertiseCallback = callback
        advertiser.startAdvertisingSet(params, data, null, null, null, callback)
    }

    @SuppressLint("MissingPermission")
    private fun stopAdvertising(result: MethodChannel.Result) {
        // §5.1: reporting success for a stop the radio refused would claim an
        // invisibility that does not exist, and nothing above can detect it.
        if (advertiseCallback != null && advertiser == null) {
            return fail(result, "unavailable", "Bluetooth went away before advertising stopped")
        }
        stopAdvertisingInternal()
        ok(result)
    }

    @SuppressLint("MissingPermission")
    private fun stopAdvertisingInternal() {
        advertiseCallback?.let {
            // The R0-F2 line. A UI that merely hides is a fail, so the moment
            // the radio is actually told to stop is worth being able to point
            // at — and worth being able to see the absence of.
            Log.i(TAG, "stopping the advertisement")
            advertiser?.stopAdvertisingSet(it)
        }
        advertiseCallback = null
    }

    // ── scanning ────────────────────────────────────────────────────────────

    @SuppressLint("MissingPermission")
    private fun startScanning(result: MethodChannel.Result) {
        val scanner = scanner
            ?: return fail(result, "unavailable", "Bluetooth is off or LE scanning unsupported")
        if (scanCallback != null) return ok(result)
        // Built outside the lock; registered under it.

        val callback = object : ScanCallback() {
            override fun onScanResult(callbackType: Int, sr: ScanResult?) {
                val data = sr?.scanRecord?.getServiceData(SERVICE_PARCEL) ?: return
                val advert = BleFraming.unframe(data) ?: return
                val now = System.currentTimeMillis()
                val previous = seen.put(advert.id, Sighting(sr.device, advert, now))
                if (BleFraming.isNews(previous?.advert, advert)) {
                    // Only on news, so this stays bounded — an advertisement
                    // arrives many times a second. The psm here is what the
                    // peer *claims*; the psm it logged opening is what it has.
                    // A mismatch is the whole question.
                    Log.i(
                        TAG,
                        "sighted ${advert.id} on psm ${advert.psm}, " +
                            "${advert.payload.size} byte payload"
                    )
                    emit(
                        mapOf(
                            "type" to EVENT_PEER_FOUND,
                            "peer" to advert.id,
                            "payload" to advert.payload
                        )
                    )
                }
            }

            override fun onScanFailed(errorCode: Int) {
                emitAvailability("scan failed (code $errorCode)")
            }
        }
        // Hardware filtering: an unfiltered scan wakes the CPU for every BLE
        // device in the room, which is the battery profile N4 rules out.
        val filters = listOf(ScanFilter.Builder().setServiceUuid(SERVICE_PARCEL).build())
        // Without setLegacy(false) the scanner reports *only* legacy
        // advertisements — it would never see the extended ones we transmit,
        // and the symptom is two working radios that cannot find each other.
        val settings = ScanSettings.Builder()
            .setScanMode(ScanSettings.SCAN_MODE_LOW_LATENCY)
            .setLegacy(false)
            .setPhy(ScanSettings.PHY_LE_ALL_SUPPORTED)
            .build()
        scanCallback = callback
        // Withdrawn on failure rather than left registered. A `scanCallback`
        // with no scan behind it makes every later `startScanning` return
        // success at the guard above without registering anything — discovery
        // off, and the core told twice that it is on.
        if (runCatching { scanner.startScan(filters, settings, callback) }.isFailure) {
            scanCallback = null
            return fail(result, "io", "the radio refused to start scanning")
        }
        ok(result)
    }

    @SuppressLint("MissingPermission")
    private fun stopScanning(result: MethodChannel.Result) {
        scanCallback?.let { runCatching { scanner?.stopScan(it) } }
        scanCallback = null
        ok(result)
    }

    /**
     * Bring up an LE link to [device] before dialling it.
     *
     * An L2CAP channel needs an ACL underneath it. Raising it here rather than
     * letting the CoC request do it states the transport explicitly and gives
     * the failure a name: a refused link reports a GATT `status`, where the
     * CoC path reports only `BluetoothSocketException` code 0 for every cause
     * there is. Services are deliberately not discovered — the link is the
     * entire point, and GATT traffic we do not need is airtime we do not want
     * (R0-N4).
     *
     * **This did not fix the dial failure of §5.0.15**, and the theory it was
     * built on — that the CoC request went out with an unresolved address type
     * because every peer reads `DEVICE_TYPE_UNKNOWN` — was wrong. The link came
     * up correctly on every attempt and Android's GATT layer tore it back down,
     * out of transport control blocks. Kept for the diagnostics above, not
     * because it made anything work.
     *
     * Held for the life of the pipe rather than dropped once the channel is up:
     * the ACL survives either way, but tying it to [Pipe] means one owner and
     * one close. A `BluetoothGatt` that is never closed leaks a client
     * interface, and the stack has about 32 — leak them and *every* connection
     * on the device fails, long after the code that leaked them ran.
     */
    @SuppressLint("MissingPermission")
    private fun raiseLeLink(peer: String, device: BluetoothDevice): BluetoothGatt? {
        val settled = CountDownLatch(1)
        val link = AtomicReference<BluetoothGatt?>(null)
        val callback = object : BluetoothGattCallback() {
            override fun onConnectionStateChange(gatt: BluetoothGatt, status: Int, state: Int) {
                if (state == BluetoothProfile.STATE_CONNECTED) {
                    link.set(gatt)
                    settled.countDown()
                } else if (state == BluetoothProfile.STATE_DISCONNECTED) {
                    // status is the one number that says *why*, and it is the
                    // only place this failure is named at all.
                    Log.w(TAG, "le link to $peer went down (status $status)")
                    settled.countDown()
                }
            }
        }
        val gatt = device.connectGatt(context, false, callback, BluetoothDevice.TRANSPORT_LE)
        if (gatt == null) {
            Log.w(TAG, "the stack refused an le link to $peer")
            return null
        }
        val answered = settled.await(LINK_TIMEOUT_MS, TimeUnit.MILLISECONDS)
        if (answered && link.get() != null) return gatt
        // Refused and unanswered are different failures, and reporting a
        // refusal as a timeout sends the next reader looking for a peer that
        // was never slow — it said no, and said why, in the line above.
        Log.w(
            TAG,
            if (answered) "no le link to $peer: the stack turned it down"
            else "no le link to $peer: nothing answered in ${LINK_TIMEOUT_MS}ms"
        )
        // close, not disconnect: a gatt that never connected still holds the
        // client interface, and disconnect alone does not give it back.
        runCatching { gatt.close() }
        return null
    }

    /**
     * The one number that names why a dial was refused.
     *
     * `BluetoothSocketException`'s *message* is the same sentence — "A
     * Bluetooth Socket failure occurred" — for a PSM nobody is listening on, a
     * link that never came up, and a peer demanding authentication we do not
     * do. Its code tells those apart, and nothing else in the system does.
     *
     * API 33+ only, and `minSdk` is 29, so the class is touched solely inside
     * the version check — an unguarded reference would be a class that does not
     * exist on a device we support.
     */
    private fun errorCode(e: Exception): String {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU) return ""
        return SocketError.of(e)
    }

    /**
     * Holds the only reference to a class that does not exist below API 33.
     *
     * `BluetoothSocketException` is API 33+ and `minSdk` is 29. ART resolves
     * lazily, so the version check in [errorCode] almost certainly suffices on
     * its own — but "almost certainly" is carrying real weight in that
     * sentence, and there is no API 29–32 device here to settle it on. A
     * separate class costs nothing and means no method that can run on an older
     * device carries the reference at all.
     */
    @TargetApi(Build.VERSION_CODES.TIRAMISU)
    private object SocketError {
        fun of(e: Exception): String =
            (e as? BluetoothSocketException)?.let { " (code ${it.errorCode})" } ?: ""
    }

    /** Ages out sightings; the radio has no "gone" event. */
    private val ageSweep = object : Runnable {
        override fun run() {
            val cutoff = System.currentTimeMillis() - PEER_TTL_MS
            seen.entries.removeIf { (id, s) ->
                // A peer we hold a pipe to is still here whatever the
                // advertisement says.
                val stale = s.lastSeen < cutoff && !pipes.containsKey(id)
                if (stale) emit(mapOf("type" to EVENT_PEER_LOST, "peer" to id))
                stale
            }
            main.postDelayed(this, AGE_SWEEP_MS)
        }
    }

    // ── pipes (L2CAP CoC) ───────────────────────────────────────────────────

    @SuppressLint("MissingPermission")
    private fun openServerSocket(): Boolean {
        val adapter = adapter ?: run {
            Log.w(TAG, "no listener: Bluetooth is unavailable")
            return false
        }
        return try {
            val socket = adapter.listenUsingInsecureL2capChannel()
            serverSocket = socket
            localPsm = socket.psm
            // Rolled back rather than left half-open. `startAdvertising` skips
            // opening a listener whenever `serverSocket` is non-null, so a
            // socket kept without an accept loop would go on to publish a psm
            // that answers nothing — a dial that fails every time from the far
            // side, with nothing here to say why. That is the failure the loop
            // below warns about, reached from the other direction.
            if (!submit { acceptLoop(socket) }) {
                serverSocket = null
                localPsm = 0
                runCatching { socket.close() }
                Log.w(TAG, "could not open an L2CAP listener: the radio is shutting down")
                return false
            }
            Log.i(TAG, "listening on psm $localPsm")
            true
        } catch (e: IOException) {
            Log.w(TAG, "could not open an L2CAP listener: ${e.message}")
            false
        } catch (e: SecurityException) {
            Log.w(TAG, "could not open an L2CAP listener: not permitted")
            false
        }
    }

    private fun acceptLoop(socket: BluetoothServerSocket) {
        // Captured now: reading `psm` off a closed socket is not reliable, and
        // the number is most wanted precisely when the loop is ending.
        val psm = localPsm
        while (!shuttingDown.get()) {
            val client = try {
                socket.accept()
            } catch (e: IOException) {
                // Worth distinguishing from the orderly exit below. A loop that
                // ends here while we are still advertising leaves us publishing
                // a psm nothing will ever answer on — which from the far side
                // is a dial that fails every time, with nothing on this side
                // to say why.
                Log.i(TAG, "stopped accepting on psm $psm: ${e.message}")
                return // socket closed by shutdown
            }
            Log.i(TAG, "accepted a dial on psm $psm")
            // The dialer's id is not on the socket — it arrives as the first
            // line, mirroring what it advertises. Reading it off-loop keeps one
            // slow peer from wedging every accept (the LAN rung's slowloris).
            // Not left dangling if shutdown beat us to it: an accepted socket
            // nobody will ever read is a peer holding a pipe we have forgotten.
            if (!submit { adoptInbound(client) }) runCatching { client.close() }
        }
    }

    private fun adoptInbound(socket: BluetoothSocket) {
        val id = try {
            readHello(socket)
        } catch (e: IOException) {
            // An accept that gets this far and then dies is a *different*
            // failure from a dial that never connected, and from the far side
            // the two are indistinguishable — both are "the pipe failed".
            Log.w(TAG, "an accepted dial never said who it was: ${e.message}")
            runCatching { socket.close() }
            return
        }
        if (id == null) {
            Log.w(TAG, "an accepted dial introduced itself with an id we refuse")
            runCatching { socket.close() }
            return
        }
        Log.i(TAG, "accepted $id")
        openPipe(id, socket)
    }

    /**
     * Hello is `[len:1][id]`, written by the dialer immediately on connect.
     *
     * `BluetoothSocket` exposes no read timeout, so a peer that connects and
     * says nothing holds this thread. That is survivable only because the
     * handshake runs off the accept loop — the LAN rung wedged every accept for
     * ~17 minutes on exactly this shape before it was moved off.
     */
    private fun readHello(socket: BluetoothSocket): String? {
        val input = socket.inputStream
        val len = input.read()
        if (len <= 0 || len > BleFraming.MAX_ID) return null
        val buf = ByteArray(len)
        var read = 0
        while (read < len) {
            val n = input.read(buf, read, len - read)
            if (n < 0) return null
            read += n
        }
        // Validated, not merely decoded: an id the core rejects would leave us
        // holding a socket and a reader thread for a pipe it never hears about.
        return BleFraming.helloId(buf)
    }

    @SuppressLint("MissingPermission")
    private fun connect(peer: String, result: MethodChannel.Result) {
        // Refused rather than accepted-then-dropped: §6 rule 2 owes a
        // pipeOpened or pipeFailed for every accepted dial, and a shut-down
        // adapter emits neither (rule 6). Saying no is the only answer that
        // keeps both rules.
        if (shuttingDown.get()) return fail(result, "unavailable", "the radio is shut down")
        val sighting = seen[peer] ?: run {
            Log.w(TAG, "cannot dial $peer: not among ${seen.size} sighting(s)")
            return fail(result, "no_such_peer", "$peer has not been seen")
        }
        if (pipes.containsKey(peer)) return ok(result) // core re-announces (§6)
        if (!dialling.add(peer)) {
            // Accepted, and deliberately silent: the dial already running will
            // emit pipeOpened or pipeFailed, and the core acts on one of those
            // per peer either way (§6).
            Log.i(TAG, "already dialling $peer, letting the first attempt stand")
            return ok(result)
        }

        // Acceptance only: the outcome arrives as pipeOpened / pipeFailed.
        ok(result)
        val psm = sighting.advert.psm
        // How old the sighting is separates a live peer from a remembered one:
        // a psm read from a stale advertisement points at a listener that may
        // no longer exist, and the dial fails identically either way.
        val age = System.currentTimeMillis() - sighting.lastSeen
        // Bond state and transport type, because an insecure L2CAP channel is
        // supposed to need neither — if a refusal turns out to be about
        // authentication, these are what say so. Neither is an address, so
        // neither survives the id rotation as a correlator (R0-F2).
        val bond = sighting.device.bondState
        val type = sighting.device.type
        val device = sighting.device
        // Devices with a GATT connection — which is narrower than "every LE
        // link", but is the pool that actually runs out. Android allocates a
        // GATT control block for *every* LE ACL, not only for app GATT clients;
        // that is precisely why a plain L2CAP dial died of it. So the profile
        // count tracks the contended resource even though it is not a count of
        // ACLs. GATT_MAX_PHY_CHANNEL is commonly 7 and is shared with every app
        // on the phone: Fast Pair, Find Hub, a watch. Full, and the stack lets
        // our connection complete and then tears the ACL straight back down —
        // "out of resources", visible only in the system log, surfacing here as
        // a bare status 133. Eight runs went into finding that; this is the
        // number that says it at a glance.
        val gattLinks = runCatching { manager.getConnectedDevices(BluetoothProfile.GATT).size }
            .getOrDefault(-1)
        // Shutdown can still land between the guard above and here; then the
        // dial is dropped and `dialling` has to be cleared by hand, because the
        // `finally` that normally does it lives inside work that never ran.
        val queued = submit {
            Log.i(
                TAG,
                "dialling $peer on psm $psm (last seen ${age}ms ago, bond $bond, " +
                    "type $type, $gattLinks gatt connection(s) already up)"
            )
            // Held here until the pipe adopts it, so a dial that fails anywhere
            // below still gives the client interface back (see [raiseLeLink]).
            var link: BluetoothGatt? = null
            try {
                val started = System.currentTimeMillis()
                link = raiseLeLink(peer, device)
                    ?: throw IOException(linkFailureReason(gattLinks))
                Log.i(TAG, "le link to $peer up in ${System.currentTimeMillis() - started}ms")
                val socket = device.createInsecureL2capChannel(psm)
                socket.connect()
                socket.outputStream.apply {
                    write(BleFraming.hello(localId ?: ""))
                    flush()
                }
                Log.i(TAG, "dialled $peer on psm $psm")
                openPipe(peer, socket, link)
                link = null // the pipe owns it now
            } catch (e: Exception) {
                // The class carries as much as the message. An IOException out
                // of `connect` is the remote refusing the psm; a
                // SecurityException is our own permissions; anything else is
                // the device object, not the link. All three reach the core as
                // the same `pipeFailed`.
                Log.w(
                    TAG,
                    "dial to $peer on psm $psm failed: ${e.javaClass.simpleName}" +
                        "${errorCode(e)}: ${e.message}"
                )
                emit(mapOf("type" to EVENT_PIPE_FAILED, "peer" to peer, "why" to (e.message ?: "dial failed")))
            } finally {
                // Still set means the pipe never adopted it, so this is the
                // only chance to give the client interface back.
                link?.let { runCatching { it.close() } }
                // Released on both paths. Left set after a failure, this peer
                // would be undialable for the life of the process — trading a
                // failure that retries for one that never does.
                dialling.remove(peer)
            }
        }
        if (!queued) dialling.remove(peer)
    }

    /**
     * How the core names us (§5.5). Set before any advertisement and on every
     * rotation, because a dialer introduces itself by this id even with
     * Discovery off — and never by its MAC, which Android rotates underneath us.
     */
    @Volatile
    private var localId: String? = null

    private fun openPipe(peer: String, socket: BluetoothSocket, link: BluetoothGatt? = null) {
        val pipe = Pipe(socket, link)
        val existing = pipes.put(peer, pipe)
        if (existing != null) {
            // Simultaneous dial. Report the open regardless — the core opens
            // one pipe (§6) — but do not leak the socket we displaced.
            //
            // Logged because it is the case we cannot otherwise count: both
            // ends dialling is ordinary here (nothing decides who dials, unlike
            // who opens the handshake), and how often it lands is the
            // difference between a rare race and the normal opening.
            Log.i(TAG, "simultaneous dial with $peer: keeping the newer pipe")
            existing.release()
        }
        // `shutdown` closes one snapshot of `pipes`. A pipe landing after that
        // snapshot is past everything that would ever close it, and it now owns
        // an LE link as well as a socket — so it is checked here rather than
        // left to the teardown that has already been and gone.
        if (shuttingDown.get()) {
            if (pipes.remove(peer, pipe)) pipe.release()
            return
        }
        emit(mapOf("type" to EVENT_PIPE_OPENED, "peer" to peer))
        // Emitted before the reader is queued, so `received` can never reach
        // the core ahead of the `pipeOpened` it belongs to. If the reader then
        // cannot start, the pipe is withdrawn and the open is closed off rather
        // than left dangling: a pipe in the map with nobody reading it is a
        // peer we believe we can hear and cannot.
        if (!submit { readLoop(peer, socket) }) {
            // Reachable only once shutdown has begun: `submit` refuses for no
            // other reason, and a cached pool rejects for no other reason. So
            // no `pipeClosed` — §6 rule 6 says the adapter goes silent, and
            // that would be a callback into a core that has asked us to stop.
            // The `pipeOpened` above stands: it went out before shutdown was
            // observed, and a second event now would not unsay it.
            if (pipes.remove(peer, pipe)) pipe.release()
        }
    }

    private fun readLoop(peer: String, socket: BluetoothSocket) {
        val buf = ByteArray(4096)
        try {
            while (true) {
                val n = socket.inputStream.read(buf)
                if (n < 0) break
                if (n > 0) {
                    emit(mapOf("type" to EVENT_RECEIVED, "peer" to peer, "bytes" to buf.copyOf(n)))
                }
            }
        } catch (e: IOException) {
            // Fall through: a severed link and a clean hang-up are the same
            // event to the core.
        }
        // Only report the close if this socket is still the live one — a
        // re-dial may have replaced it while this reader was blocked. Removed
        // by identity, so a pipe that replaced ours between the read and the
        // remove is not the one torn down: that would strand its LE link with
        // nothing left holding a reference to close it.
        val mine = pipes[peer]
        if (mine?.socket === socket && pipes.remove(peer, mine)) {
            mine.release()
            emit(mapOf("type" to EVENT_PIPE_CLOSED, "peer" to peer))
        }
    }

    private fun send(peer: String, bytes: ByteArray, result: MethodChannel.Result) {
        val pipe = pipes[peer] ?: return fail(result, "no_such_peer", "no pipe to $peer")
        ok(result)
        // Same double-reply shape as `connect`: answered above, queued here.
        submit {
            try {
                // §5.2: one send is one contiguous ordered write.
                synchronized(pipe.writeLock) {
                    pipe.socket.outputStream.write(bytes)
                    pipe.socket.outputStream.flush()
                }
                // §5.3: without this the core's window closes and the pipe
                // wedges after ~64 kB.
                emit(mapOf("type" to EVENT_WRITE_COMPLETE, "peer" to peer, "bytes" to bytes.size))
            } catch (e: IOException) {
                closePipe(peer, report = true)
            }
        }
    }

    private fun closePipe(peer: String, report: Boolean) {
        val pipe = pipes.remove(peer) ?: return
        pipe.release()
        if (report) emit(mapOf("type" to EVENT_PIPE_CLOSED, "peer" to peer))
    }

    // ── availability and teardown ───────────────────────────────────────────

    /** This device's permission state, as [BlePermissions] sees it. */
    private fun permissions(): BlePermissions.State = BlePermissions.state {
        context.checkSelfPermission(it) == PackageManager.PERMISSION_GRANTED
    }

    /**
     * Why the radio cannot be used, or `null` if it can.
     *
     * Permission is checked before the adapter, and not only because it is the
     * one the user can do something about: from Android 12 `isEnabled` itself
     * requires `BLUETOOTH_CONNECT`, so an unpermitted app asking whether
     * Bluetooth is on is asking a question it has no right to.
     */
    private fun unusable(): String? {
        BlePermissions.reason(permissions())?.let { return it }
        return if (adapter?.isEnabled == true) null else "Bluetooth is off"
    }

    /**
     * Re-report availability. The host calls this when it has changed something
     * the adapter cannot observe — a permission granted in a dialog, or in
     * Settings while the app was away. Nothing broadcasts either.
     */
    fun refreshAvailability() = emitAvailability()

    /**
     * Say whether the radio can be used, and if not, why.
     *
     * `reason` defaults to the live answer. Pass one only to report something a
     * re-check would miss: a `SecurityException` that has already happened, or
     * a `TURNING_OFF` broadcast that arrives before `isEnabled` flips.
     *
     * There is deliberately no way to force *available*. Before this checked
     * permissions, Bluetooth being on was reported as the radio being usable —
     * which on a device that had never been asked for the three permissions was
     * simply untrue, and left "nobody is nearby" as the only symptom.
     */
    private fun emitAvailability(reason: String? = unusable()) {
        Log.i(TAG, "radio " + (reason?.let { "unavailable: $it" } ?: "available"))
        emit(mapOf("type" to EVENT_AVAILABILITY, "available" to (reason == null), "reason" to reason))
    }

    private val bluetoothStateReceiver = object : BroadcastReceiver() {
        override fun onReceive(ctx: Context?, intent: Intent?) {
            if (intent?.action != BluetoothAdapter.ACTION_STATE_CHANGED) return
            when (intent.getIntExtra(BluetoothAdapter.EXTRA_STATE, BluetoothAdapter.ERROR)) {
                // Recomputed rather than asserted: Bluetooth coming back on a
                // device that never granted the permissions is not the radio
                // becoming usable.
                BluetoothAdapter.STATE_ON -> emitAvailability()
                BluetoothAdapter.STATE_OFF, BluetoothAdapter.STATE_TURNING_OFF -> {
                    // The radio takes every pipe with it; say so rather than
                    // leaving the core believing in links that no longer exist.
                    pipes.keys.toList().forEach { closePipe(it, report = true) }
                    seen.clear()
                    // The listener goes with them. It is bound to a Bluetooth
                    // stack that is being torn down, and a stale one is worse
                    // than none: `startAdvertising` skips opening a listener
                    // when this is non-null, so the rung would come back
                    // advertising a PSM nothing is accepting on — a peer that
                    // is visible and undialable.
                    //
                    // Every call here is wrapped: this is a broadcast
                    // receiver, so unlike `onMethodCall` there is no `catch`
                    // above it, and a stack mid-teardown is exactly where a
                    // `SecurityException` or a dead-object throw is plausible.
                    // Taking the app down while the user toggles Bluetooth
                    // would be a worse bug than the one being fixed.
                    runCatching { serverSocket?.close() }
                    serverSocket = null
                    localPsm = 0
                    runCatching { stopAdvertisingInternal() }
                    // Not stopped, only forgotten: the scanner is gone with the
                    // stack, and a non-null callback would make `startScanning`
                    // return success without re-registering when it comes back.
                    scanCallback = null
                    emitAvailability("Bluetooth is off")
                }
            }
        }
    }

    fun shutdown() {
        if (!shuttingDown.compareAndSet(false, true)) return
        stopAdvertisingInternal()
        scanCallback?.let { runCatching { scanner?.stopScan(it) } }
        scanCallback = null
        runCatching { serverSocket?.close() }
        serverSocket = null
        pipes.keys.toList().forEach { closePipe(it, report = false) }
        seen.clear()
        main.removeCallbacks(ageSweep)
        io.shutdownNow()
    }
}
