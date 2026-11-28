import com.android.build.api.variant.FilterConfiguration

plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.compose)
}

val splitApks = providers.gradleProperty("splitApks").map { it.toBoolean() }.getOrElse(false)
val releaseVersionName = providers.gradleProperty("versionName").getOrElse("1.0.0")
val releaseVersionCode = providers.gradleProperty("versionCode").map { it.toInt() }.getOrElse(1)

// providers.environmentVariable (not System.getenv) so Gradle tracks the values as configuration-cache inputs.
fun envOrNull(name: String): String? = providers.environmentVariable(name).orNull?.takeIf { it.isNotBlank() }

val ksPath = envOrNull("RUSTYGRADE_KEYSTORE_PATH")?.let(::file)?.takeIf { it.isFile }
val ksPass = envOrNull("RUSTYGRADE_KEYSTORE_PASSWORD")
val ksAlias = envOrNull("RUSTYGRADE_KEY_ALIAS")
val ksKeyPass = envOrNull("RUSTYGRADE_KEY_PASSWORD")
val hasReleaseSigning = ksPath != null && ksPass != null && ksAlias != null && ksKeyPass != null

// Output files are named after the app (rustygrade-arm64-v8a-release.apk, rustygrade-release.aab).
// Note: applicationId is dev.okarin.rustygrade, so old dev.okarin.retrograde installs cannot upgrade
// in place and appear as a separate app; users can migrate playlists, stats, and settings via Backup ZIP.
base {
    archivesName.set("rustygrade")
}

android {
    namespace = "dev.okarin.rustygrade"
    ndkVersion = "27.3.13750724"
    // AndroidX 2026 releases require compileSdk >= 37.1; targetSdk intentionally stays 35.
    compileSdk {
        version = release(37) { minorApiLevel = 2 }
    }

    defaultConfig {
        applicationId = "dev.okarin.rustygrade"
        minSdk = 28
        targetSdk = 35
        versionCode = releaseVersionCode
        versionName = releaseVersionName
        if (!splitApks) {
            ndk { abiFilters += listOf("arm64-v8a", "x86_64") }
        }
    }

    // The app only ships English strings; drop the 80+ library locales AndroidX/Media3 would add.
    androidResources {
        localeFilters += "en"
    }

    splits {
        abi {
            isEnable = splitApks
            reset()
            include("arm64-v8a", "x86_64")
            isUniversalApk = true
        }
    }

    signingConfigs {
        if (hasReleaseSigning) {
            create("release") {
                storeFile = ksPath
                storePassword = ksPass
                keyAlias = ksAlias
                keyPassword = ksKeyPass
            }
        }
    }

    buildTypes {
        debug {
            // Installs beside a release build (different id and signing key): testing a debug build
            // never needs the release uninstalled, so the user's library data survives.
            applicationIdSuffix = ".dev"
            versionNameSuffix = "-dev"
        }
        release {
            isMinifyEnabled = true
            isShrinkResources = true
            proguardFiles(
                getDefaultProguardFile("proguard-android-optimize.txt"),
                "proguard-rules.pro",
            )
            if (hasReleaseSigning) {
                signingConfig = signingConfigs.getByName("release")
            }
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    buildFeatures { compose = true }

    lint {
        // lintDebug is clean (0 errors, 0 warnings): keep it that way.
        warningsAsErrors = true
        // Dependency-freshness checks flip as new library versions ship, not when our code changes; version bumps
        // are handled deliberately, so they must not turn CI red on their own.
        disable += setOf("GradleDependency", "NewerVersionAvailable", "AndroidGradlePluginVersion")
    }

    packaging {
        jniLibs {
            useLegacyPackaging = false
            // librustygrade_ffi.so exists only for arm64-v8a and x86_64; the universal split APK must not
            // carry JNA / AndroidX natives for ABIs the app cannot run on (abiFilters is off with splits).
            excludes += setOf("lib/armeabi/**", "lib/armeabi-v7a/**", "lib/x86/**", "lib/mips/**", "lib/mips64/**")
        }
        resources {
            // kotlinx-coroutines debug agent blob and AndroidX/Media3 `*.version` markers: dead weight in the APK.
            excludes += setOf("DebugProbesKt.bin", "META-INF/*.version")
        }
    }
}

// Multi-APK versionCode scheme (only when -PsplitApks=true): every APK of one release gets a distinct,
// store-ordered code of `base * 10 + offset` (universal < x86_64 < arm64-v8a), so a device-specific split
// always outranks the universal APK. The AAB (bundleRelease, splitApks=false) keeps the exact base code.
androidComponents {
    onVariants { variant ->
        if (!splitApks) return@onVariants
        val abiVersionOffsets = mapOf("x86_64" to 1, "arm64-v8a" to 2)
        variant.outputs.forEach { output ->
            val abi = output.filters
                .firstOrNull { it.filterType == FilterConfiguration.FilterType.ABI }
                ?.identifier
            output.versionCode.set(releaseVersionCode * 10 + (abiVersionOffsets[abi] ?: 0))
        }
    }
}

kotlin {
    jvmToolchain(17)
}

dependencies {
    implementation(platform(libs.compose.bom))

    implementation(libs.androidx.core.ktx)
    implementation(libs.androidx.activity.compose)
    implementation(libs.androidx.lifecycle.runtime.ktx)
    implementation(libs.androidx.lifecycle.runtime.compose)
    implementation(libs.androidx.lifecycle.viewmodel.compose)
    implementation(libs.compose.ui)
    implementation(libs.compose.foundation)
    implementation(libs.compose.material3)
    implementation(libs.compose.ui.tooling.preview)
    debugImplementation(libs.compose.ui.tooling)

    implementation(libs.media3.exoplayer)
    implementation(libs.media3.session)
    implementation(libs.media3.common)
    implementation(libs.coil.compose)
    implementation(libs.kotlinx.coroutines.android)

    implementation(libs.jna) { artifact { type = "aar" } }
}
