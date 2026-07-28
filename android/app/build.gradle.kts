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
        // R0-N7 sets the floor at Android 10, and that is also exactly where
        // BluetoothDevice.createInsecureL2capChannel arrives (API 29). Pinning
        // it here is what lets the BLE rung use L2CAP CoC unconditionally and
        // skip a GATT fallback in Ring 0 — see docs/BLE_CHANNEL.md.
        minSdk = 29
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
