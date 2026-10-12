plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("org.jetbrains.kotlin.plugin.compose")
}

android {
    namespace = "com.rvah.tv"
    compileSdk = 36

    defaultConfig {
        applicationId = "com.rvah.tv"
        minSdk = 23
        targetSdk = 36
        versionCode = 1
        versionName = "0.1.0"
        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
        ndk {
            abiFilters += listOf("arm64-v8a", "armeabi-v7a", "x86_64")
        }
        // Ed25519 keys (hex, comma-separated) whose signed .rvah packages this
        // build may install: -Prvah.trustedKeys=<hex> or ~/.gradle/gradle.properties.
        // Filtered to hex digits and commas so the value cannot inject code.
        val trustedKeys = (project.findProperty("rvah.trustedKeys") as String?).orEmpty()
            .filter { it in '0'..'9' || it.lowercaseChar() in 'a'..'f' || it == ',' }
        buildConfigField("String", "TRUSTED_KEYS", "\"$trustedKeys\"")
        buildConfigField("String", "DEV_SECRETS", "\"\"")
    }

    buildTypes {
        debug {
            applicationIdSuffix = ".debug"
            // Add-on API keys from the repository's .env (gitignored), which
            // the debug app stores in the matching add-on's vault when unset;
            // see DevSecrets.kt. Only TMDB_*, MDBLIST_*, TRAKT_*, PROWLARR_*, REAL_DEBRID_* and SUBDL_* variables,
            // with values filtered to token characters so they cannot inject
            // code. Release builds get none.
            val env = rootProject.projectDir.parentFile.resolve(".env")
            val devSecrets = if (env.isFile) {
                env.readLines()
                    .map { it.trim() }
                    .filter { it.isNotEmpty() && !it.startsWith("#") && '=' in it }
                    .map { it.removePrefix("export ").substringBefore('=').trim() to it.substringAfter('=').trim().trim('"', '\'') }
                    .filter { (name, value) -> Regex("(TMDB|MDBLIST|TRAKT|PROWLARR|REAL_DEBRID|SUBDL)_[A-Z0-9_]+").matches(name) && value.isNotEmpty() }
                    .joinToString(";") { (name, value) -> "$name=" + value.filter { it.isLetterOrDigit() || it in "._-" } }
            } else {
                ""
            }
            buildConfigField("String", "DEV_SECRETS", "\"$devSecrets\"")
        }
        release {
            isMinifyEnabled = true
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro")
        }
    }

    buildFeatures {
        compose = true
        buildConfig = true
    }

    sourceSets["main"].java.srcDir("src/main/generated")

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    kotlinOptions.jvmTarget = "17"

    packaging.resources.excludes += "/META-INF/{AL2.0,LGPL2.1}"
}

dependencies {
    val composeBom = platform("androidx.compose:compose-bom:2025.05.01")
    implementation(composeBom)
    androidTestImplementation(composeBom)

    implementation("androidx.activity:activity-compose:1.10.1")
    implementation("androidx.lifecycle:lifecycle-runtime-compose:2.9.1")
    implementation("androidx.lifecycle:lifecycle-viewmodel-compose:2.9.1")
    implementation("androidx.tv:tv-material:1.0.0")
    implementation("androidx.compose.material3:material3")
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.ui:ui-tooling-preview")

    val media3 = "1.11.1"
    implementation("androidx.media3:media3-exoplayer:$media3")
    implementation("androidx.media3:media3-exoplayer-hls:$media3")
    implementation("androidx.media3:media3-exoplayer-dash:$media3")
    implementation("androidx.media3:media3-session:$media3")
    implementation("androidx.media3:media3-ui-compose:$media3")

    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.10.2")
    // Posters and backdrops on Discover rows and details pages.
    implementation("io.coil-kt.coil3:coil-compose:3.2.0")
    implementation("io.coil-kt.coil3:coil-network-okhttp:3.2.0")
    implementation("net.java.dev.jna:jna:5.17.0@aar")

    testImplementation("junit:junit:4.13.2")
    testImplementation("org.jetbrains.kotlinx:kotlinx-coroutines-test:1.10.2")
    androidTestImplementation("androidx.test.ext:junit:1.2.1")
    androidTestImplementation("androidx.test.espresso:espresso-core:3.6.1")
    androidTestImplementation("androidx.compose.ui:ui-test-junit4")
    debugImplementation("androidx.compose.ui:ui-tooling")
    debugImplementation("androidx.compose.ui:ui-test-manifest")
}
