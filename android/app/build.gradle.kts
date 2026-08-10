plugins {
    id("com.android.application")
    // The Flutter Gradle Plugin must be applied after the Android and Kotlin Gradle plugins.
    id("dev.flutter.flutter-gradle-plugin")
}

android {
    namespace = "org.hoppler.hoppler"
    compileSdk = flutter.compileSdkVersion
    ndkVersion = flutter.ndkVersion

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    defaultConfig {
        // TODO: Specify your own unique Application ID (https://developer.android.com/studio/build/application-id.html).
        applicationId = "org.hoppler.hoppler"
        // You can update the following values to match your application needs.
        // For more information, see: https://flutter.dev/to/review-gradle-config.
        // R0-N7 sets the floor at Android 12 (amended 10 Aug 2026). Below it the
        // BLE rung cannot run at all: the modern Bluetooth permissions do not
        // exist, and a scan instead requires ACCESS_FINE_LOCATION, which Hoppler
        // does not ask for.
        //
        // Secondary, and the reason the floor used to be 29:
        // BluetoothDevice.createInsecureL2capChannel arrives at API 29, so L2CAP
        // CoC is available unconditionally and Ring 0 needs no GATT fallback —
        // see docs/BLE_CHANNEL.md. That holds at any floor from 29 up, so
        // raising it costs nothing here.
        //
        // Costing of all three options, and what the floor excludes: the ring-0
        // findings, T08b §5.0.23 (outside this repo).
        minSdk = 31
        targetSdk = flutter.targetSdkVersion
        versionCode = flutter.versionCode
        versionName = flutter.versionName
    }

    buildTypes {
        release {
            // TODO: Add your own signing config for the release build.
            // Signing with the debug keys for now, so `flutter run --release` works.
            signingConfig = signingConfigs.getByName("debug")
        }
    }
}

tasks.withType<Test>().configureEach {
    // A test task that discovers nothing still reports BUILD SUCCESSFUL, which
    // reads as "all green" when it means "nothing ran" — so make the absence of
    // tests a failure rather than something a human has to notice in a log.
    failOnNoDiscoveredTests = true
    testLogging { events("passed", "failed", "skipped") }
}

dependencies {
    // JVM unit tests for the adapter's radio-free logic (framing, id
    // validation, the sighting decision). BLE behaviour still needs two
    // phones; parsing a byte array does not.
    testImplementation("junit:junit:4.13.2")
}

kotlin {
    compilerOptions {
        jvmTarget = org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17
    }
}

flutter {
    source = "../.."
}
