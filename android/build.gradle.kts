import org.jetbrains.kotlin.gradle.dsl.JvmTarget

plugins {
    id("com.android.library")
    id("org.jetbrains.kotlin.android")
}

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
            val kotlinVersion = (findProperty("kotlinVersion") as String?)
                ?: rootProject.findProperty("kotlinVersion") as String?
                ?: rootProject.buildscript.configurations
                    .getByName("classpath").resolvedConfiguration.resolvedArtifacts
                    .find { it.name == "kotlin-gradle-plugin" }
                    ?.moduleVersion?.id?.version
                ?: "2.2.10"
            classpath("org.jetbrains.kotlin:kotlin-serialization:$kotlinVersion")
        }
    }
}

// Standalone `android/settings.gradle` includes `:lib`; a Tauri host app does not.
val isStandaloneLibBuild = findProject(":lib")?.projectDir == file("lib")

if (!isStandaloneLibBuild) {
    apply(plugin = "org.jetbrains.kotlin.plugin.serialization")
}

android {
    namespace = "org.silvermine.plugin.download"
    compileSdk = 37

    defaultConfig {
        minSdk = 24

        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
        consumerProguardFiles("consumer-rules.pro")
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    if (!isStandaloneLibBuild) {
        sourceSets {
            named("main") {
                java.srcDir("lib/src/main/java")
            }
        }
    }
}

kotlin {
    compilerOptions {
        jvmTarget = JvmTarget.JVM_17
    }
}

dependencies {
    if (isStandaloneLibBuild) {
        implementation(project(":lib"))
    } else {
        implementation("androidx.core:core-ktx:1.13.1")
        implementation("androidx.work:work-runtime-ktx:2.9.1")
        implementation("com.squareup.okhttp3:okhttp:4.12.0")
        implementation("org.jetbrains.kotlinx:kotlinx-serialization-json:1.6.3")
        implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.7.3")
    }

    if (findProject(":tauri-android") != null) {
        implementation(project(":tauri-android"))
    }

    if (isStandaloneLibBuild) {
        // `:lib` scopes these as `implementation`, so they do not reach this
        // module's compile classpath, but `DownloadPlugin.kt` imports them
        // directly. Its other `:lib` dependencies need no entry here.
        implementation("org.jetbrains.kotlinx:kotlinx-serialization-json:1.6.3")
        implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.7.3")
    }
}
