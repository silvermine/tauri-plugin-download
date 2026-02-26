// In Tauri subproject builds, self-resolve the Kotlin serialization plugin
// matching the host's Kotlin version. In standalone builds, the serialization
// plugin is only needed by :lib (which applies it via settings.gradle).
buildscript {
    if (rootProject.projectDir != projectDir) {
        repositories {
            mavenCentral()
            google()
        }
        dependencies {
            val kotlinVersion = rootProject.buildscript.configurations
                .getByName("classpath").resolvedConfiguration.resolvedArtifacts
                .find { it.name == "kotlin-gradle-plugin" }
                ?.moduleVersion?.id?.version ?: "1.9.25"
            classpath("org.jetbrains.kotlin:kotlin-serialization:$kotlinVersion")
        }
    }
}

plugins {
    id("com.android.library")
    id("org.jetbrains.kotlin.android")
}

if (findProject(":lib") == null) {
    apply(plugin = "org.jetbrains.kotlin.plugin.serialization")
}

android {
    namespace = "org.silvermine.plugin.download"
    compileSdk = 34

    defaultConfig {
        minSdk = 24

        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
        consumerProguardFiles("consumer-rules.pro")
    }

    buildTypes {
        release {
            isMinifyEnabled = false
            proguardFiles(
                getDefaultProguardFile("proguard-android-optimize.txt"),
                "proguard-rules.pro"
            )
        }
    }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    kotlinOptions {
        jvmTarget = "17"
    }
}

if (findProject(":lib") != null) {
    // Standalone build: depend on :lib as a separate module.
    dependencies {
        implementation(project(":lib"))
    }
} else {
    // Tauri subproject build: compile :lib sources directly.
    // Use dependency versions compatible with Kotlin 1.9.x and compileSdk 34.
    android.sourceSets["main"].java.srcDir("lib/src/main/java")
    dependencies {
        implementation("androidx.core:core-ktx:1.13.1")
        implementation("androidx.work:work-runtime-ktx:2.9.1")
        implementation("com.squareup.okhttp3:okhttp:4.12.0")
        implementation("org.jetbrains.kotlinx:kotlinx-serialization-json:1.6.3")
        implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.7.3")
    }
}

dependencies {
    implementation(project(":tauri-android"))
    if (findProject(":lib") != null) {
        implementation("org.jetbrains.kotlinx:kotlinx-serialization-json:1.7.3")
        implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.9.0")
    }
}
