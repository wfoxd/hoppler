package org.hoppler.hoppler.ble

import android.annotation.SuppressLint
import android.bluetooth.BluetoothAdapter
import android.bluetooth.BluetoothDevice
import android.bluetooth.BluetoothManager
import android.bluetooth.BluetoothServerSocket
import android.bluetooth.BluetoothSocket
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
import android.os.Handler
import android.os.Looper
import android.os.ParcelUuid
import io.flutter.plugin.common.EventChannel
import io.flutter.plugin.common.MethodCall
import io.flutter.plugin.common.MethodChannel
import java.io.IOException
import java.util.UUID
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.Executors
import java.util.concurrent.atomic.AtomicBoolean

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
        const val PEER_TTL_MS = 15_000L
        private const val AGE_SWEEP_MS = 3_000L
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

    private data class Sighting(val device: BluetoothDevice, val psm: Int, var lastSeen: Long)

    private class Pipe(val socket: BluetoothSocket) {
        /** §5.2: one `send` is one contiguous, ordered write. */
        val writeLock = Any()
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
            emitAvailability(available = false, reason = "permission denied")
        } catch (e: Exception) {
            fail(result, "io", e.message ?: e.toString())
        }
    }

    // ── advertising ─────────────────────────────────────────────────────────

    /**
     * Service data is `[psm:2][idLen:1][id][payload]`. The PSM travels in the
     * advertisement so a scanner can dial without a GATT round-trip, and the id
     * travels because the MAC address rotates underneath us (§5.4).
     */
    private fun frame(localId: String, payload: ByteArray): ByteArray {
        val id = localId.toByteArray(Charsets.US_ASCII)
        require(id.size in 1..63) { "local id must be 1..63 bytes" }
        return ByteArray(3 + id.size + payload.size).also {
            it[0] = (localPsm shr 8).toByte()
            it[1] = (localPsm and 0xFF).toByte()
            it[2] = id.size.toByte()
            id.copyInto(it, 3)
            payload.copyInto(it, 3 + id.size)
        }
    }

    private fun unframe(data: ByteArray): Triple<String, Int, ByteArray>? {
        if (data.size < 3) return null
        val psm = ((data[0].toInt() and 0xFF) shl 8) or (data[1].toInt() and 0xFF)
        val idLen = data[2].toInt() and 0xFF
        if (idLen == 0 || data.size < 3 + idLen) return null
        val id = String(data, 3, idLen, Charsets.US_ASCII)
        return Triple(id, psm, data.copyOfRange(3 + idLen, data.size))
    }

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
        val body = frame(localId, payload)
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
                    ok(result)
                } else {
                    advertiseCallback = null
                    fail(result, "io", "advertising failed (status $status)")
                }
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
        advertiseCallback?.let { advertiser?.stopAdvertisingSet(it) }
        advertiseCallback = null
    }

    // ── scanning ────────────────────────────────────────────────────────────

    @SuppressLint("MissingPermission")
    private fun startScanning(result: MethodChannel.Result) {
        val scanner = scanner
            ?: return fail(result, "unavailable", "Bluetooth is off or LE scanning unsupported")
        if (scanCallback != null) return ok(result)

        val callback = object : ScanCallback() {
            override fun onScanResult(callbackType: Int, sr: ScanResult?) {
                val data = sr?.scanRecord?.getServiceData(SERVICE_PARCEL) ?: return
                val (id, psm, payload) = unframe(data) ?: return
                val now = System.currentTimeMillis()
                val previous = seen.put(id, Sighting(sr.device, psm, now))
                // Re-sent on payload change; the core treats it as "latest
                // known", so an unchanged repeat is noise worth suppressing.
                if (previous == null || previous.psm != psm) {
                    emit(mapOf("type" to "peerFound", "peer" to id, "payload" to payload))
                }
            }

            override fun onScanFailed(errorCode: Int) {
                emitAvailability(available = false, reason = "scan failed (code $errorCode)")
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
        scanner.startScan(filters, settings, callback)
        ok(result)
    }

    @SuppressLint("MissingPermission")
    private fun stopScanning(result: MethodChannel.Result) {
        scanCallback?.let { scanner?.stopScan(it) }
        scanCallback = null
        ok(result)
    }

    /** Ages out sightings; the radio has no "gone" event. */
    private val ageSweep = object : Runnable {
        override fun run() {
            val cutoff = System.currentTimeMillis() - PEER_TTL_MS
            seen.entries.removeIf { (id, s) ->
                // A peer we hold a pipe to is still here whatever the
                // advertisement says.
                val stale = s.lastSeen < cutoff && !pipes.containsKey(id)
                if (stale) emit(mapOf("type" to "peerLost", "peer" to id))
                stale
            }
            main.postDelayed(this, AGE_SWEEP_MS)
        }
    }

    // ── pipes (L2CAP CoC) ───────────────────────────────────────────────────

    @SuppressLint("MissingPermission")
    private fun openServerSocket(): Boolean {
        val adapter = adapter ?: return false
        return try {
            val socket = adapter.listenUsingInsecureL2capChannel()
            serverSocket = socket
            localPsm = socket.psm
            io.execute { acceptLoop(socket) }
            true
        } catch (e: IOException) {
            false
        } catch (e: SecurityException) {
            false
        }
    }

    private fun acceptLoop(socket: BluetoothServerSocket) {
        while (!shuttingDown.get()) {
            val client = try {
                socket.accept()
            } catch (e: IOException) {
                return // socket closed by shutdown
            }
            // The dialer's id is not on the socket — it arrives as the first
            // line, mirroring what it advertises. Reading it off-loop keeps one
            // slow peer from wedging every accept (the LAN rung's slowloris).
            io.execute { adoptInbound(client) }
        }
    }

    private fun adoptInbound(socket: BluetoothSocket) {
        val id = try {
            readHello(socket)
        } catch (e: IOException) {
            runCatching { socket.close() }
            return
        }
        if (id == null) {
            runCatching { socket.close() }
            return
        }
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
        if (len <= 0 || len > 63) return null
        val buf = ByteArray(len)
        var read = 0
        while (read < len) {
            val n = input.read(buf, read, len - read)
            if (n < 0) return null
            read += n
        }
        return String(buf, Charsets.US_ASCII)
    }

    @SuppressLint("MissingPermission")
    private fun connect(peer: String, result: MethodChannel.Result) {
        val sighting = seen[peer]
            ?: return fail(result, "no_such_peer", "$peer has not been seen")
        if (pipes.containsKey(peer)) return ok(result) // core re-announces (§6)

        // Acceptance only: the outcome arrives as pipeOpened / pipeFailed.
        ok(result)
        io.execute {
            try {
                val socket = sighting.device.createInsecureL2capChannel(sighting.psm)
                socket.connect()
                val id = (localId ?: "").toByteArray(Charsets.US_ASCII)
                socket.outputStream.apply {
                    write(byteArrayOf(id.size.toByte()))
                    write(id)
                    flush()
                }
                openPipe(peer, socket)
            } catch (e: Exception) {
                emit(mapOf("type" to "pipeFailed", "peer" to peer, "why" to (e.message ?: "dial failed")))
            }
        }
    }

    /**
     * How the core names us (§5.5). Set before any advertisement and on every
     * rotation, because a dialer introduces itself by this id even with
     * Discovery off — and never by its MAC, which Android rotates underneath us.
     */
    @Volatile
    private var localId: String? = null

    private fun openPipe(peer: String, socket: BluetoothSocket) {
        val existing = pipes.put(peer, Pipe(socket))
        if (existing != null) {
            // Simultaneous dial. Report the open regardless — the core opens
            // one pipe (§6) — but do not leak the socket we displaced.
            runCatching { existing.socket.close() }
        }
        emit(mapOf("type" to "pipeOpened", "peer" to peer))
        io.execute { readLoop(peer, socket) }
    }

    private fun readLoop(peer: String, socket: BluetoothSocket) {
        val buf = ByteArray(4096)
        try {
            while (true) {
                val n = socket.inputStream.read(buf)
                if (n < 0) break
                if (n > 0) {
                    emit(mapOf("type" to "received", "peer" to peer, "bytes" to buf.copyOf(n)))
                }
            }
        } catch (e: IOException) {
            // Fall through: a severed link and a clean hang-up are the same
            // event to the core.
        }
        // Only report the close if this socket is still the live one — a
        // re-dial may have replaced it while this reader was blocked.
        if (pipes[peer]?.socket === socket) {
            pipes.remove(peer)
            emit(mapOf("type" to "pipeClosed", "peer" to peer))
        }
    }

    private fun send(peer: String, bytes: ByteArray, result: MethodChannel.Result) {
        val pipe = pipes[peer] ?: return fail(result, "no_such_peer", "no pipe to $peer")
        ok(result)
        io.execute {
            try {
                // §5.2: one send is one contiguous ordered write.
                synchronized(pipe.writeLock) {
                    pipe.socket.outputStream.write(bytes)
                    pipe.socket.outputStream.flush()
                }
                // §5.3: without this the core's window closes and the pipe
                // wedges after ~64 kB.
                emit(mapOf("type" to "writeComplete", "peer" to peer, "bytes" to bytes.size))
            } catch (e: IOException) {
                closePipe(peer, report = true)
            }
        }
    }

    private fun closePipe(peer: String, report: Boolean) {
        val pipe = pipes.remove(peer) ?: return
        runCatching { pipe.socket.close() }
        if (report) emit(mapOf("type" to "pipeClosed", "peer" to peer))
    }

    // ── availability and teardown ───────────────────────────────────────────

    private fun emitAvailability(
        available: Boolean = adapter?.isEnabled == true,
        reason: String? = if (adapter?.isEnabled == true) null else "Bluetooth is off"
    ) {
        emit(mapOf("type" to "availability", "available" to available, "reason" to reason))
    }

    private val bluetoothStateReceiver = object : BroadcastReceiver() {
        override fun onReceive(ctx: Context?, intent: Intent?) {
            if (intent?.action != BluetoothAdapter.ACTION_STATE_CHANGED) return
            when (intent.getIntExtra(BluetoothAdapter.EXTRA_STATE, BluetoothAdapter.ERROR)) {
                BluetoothAdapter.STATE_ON -> emitAvailability(true, null)
                BluetoothAdapter.STATE_OFF, BluetoothAdapter.STATE_TURNING_OFF -> {
                    // The radio takes every pipe with it; say so rather than
                    // leaving the core believing in links that no longer exist.
                    pipes.keys.toList().forEach { closePipe(it, report = true) }
                    seen.clear()
                    emitAvailability(false, "Bluetooth is off")
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
