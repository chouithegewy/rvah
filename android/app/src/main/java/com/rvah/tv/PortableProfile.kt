package com.rvah.tv

import androidx.activity.compose.BackHandler
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.tv.material3.Button
import androidx.tv.material3.Text
import java.io.ByteArrayOutputStream
import java.io.InputStream
import kotlinx.coroutines.launch
import uniffi.rvah_android.AndroidCore
import uniffi.rvah_android.generateProfileKey

/** Android TV enrollment for the cross-device, encrypted portable profile. */
@Composable
internal fun PortableProfileScreen(core: AndroidCore, onImported: () -> Unit, onExit: () -> Unit) {
    BackHandler(onBack = onExit)
    val context = LocalContext.current
    val resolver = context.contentResolver
    val enrollment = remember(context) { ProfileEnrollmentStore(context.applicationContext) }
    val scope = rememberCoroutineScope()
    var enrolled by remember { mutableStateOf(runCatching { enrollment.loadRecoveryKey() != null }.getOrDefault(false)) }
    var message by remember { mutableStateOf<String?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    var pendingBundle by remember { mutableStateOf<ByteArray?>(null) }
    var pendingKey by remember { mutableStateOf<String?>(null) }
    var confirmForget by remember { mutableStateOf(false) }

    fun report(result: Result<String>) {
        result.onSuccess {
            error = null
            message = it
        }.onFailure {
            message = null
            error = it.userMessage()
        }
    }

    val createBundle = rememberLauncherForActivityResult(
        ActivityResultContracts.CreateDocument("application/octet-stream"),
    ) { uri ->
        val bytes = pendingBundle
        pendingBundle = null
        if (uri != null && bytes != null) {
            scope.launch {
                report(onIo {
                    resolver.openOutputStream(uri, "rwt").use { output ->
                        requireNotNull(output) { "the selected profile document could not be opened" }
                        output.write(bytes)
                    }
                    "Encrypted profile exported. Keep its recovery key separately."
                })
            }
        }
    }
    val createKey = rememberLauncherForActivityResult(
        ActivityResultContracts.CreateDocument("text/plain"),
    ) { uri ->
        val key = pendingKey
        pendingKey = null
        if (uri != null && key != null) {
            scope.launch {
                report(onIo {
                    resolver.openOutputStream(uri, "rwt").use { output ->
                        requireNotNull(output) { "the selected key document could not be opened" }
                        output.write((key + "\n").toByteArray(Charsets.US_ASCII))
                    }
                    "Recovery key exported. Store it apart from the encrypted profile."
                })
            }
        }
    }
    val openKey = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
        if (uri != null) {
            scope.launch {
                val result = onIo {
                    val value = resolver.openInputStream(uri).use { input ->
                        requireNotNull(input) { "the selected recovery-key document could not be opened" }
                        input.bufferedReader(Charsets.US_ASCII).readText()
                    }
                    enrollment.saveRecoveryKey(value)
                    "Recovery key imported and protected by Android Keystore."
                }
                result.onSuccess {
                    enrolled = true
                    confirmForget = false
                }
                report(result)
            }
        }
    }
    val openBundle = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
        if (uri != null) {
            scope.launch {
                report(onIo {
                    val key = requireNotNull(enrollment.loadRecoveryKey()) {
                        "import or create this device's recovery key first"
                    }
                    val bytes = resolver.openInputStream(uri).use { input ->
                        requireNotNull(input) { "the selected profile document could not be opened" }
                        input.readBoundedProfile()
                    }
                    val summary = core.importProfile(bytes, key)
                    "Imported ${summary.addons} add-ons, ${summary.settings} settings, and ${summary.secrets} secrets."
                }.onSuccess { onImported() })
            }
        }
    }

    Page("Portable profile") {
        Column(
            Modifier.fillMaxWidth().verticalScroll(rememberScrollState()),
            verticalArrangement = Arrangement.spacedBy(14.dp),
        ) {
            Body(
                if (enrolled) {
                    "This device has a recovery key protected by Android Keystore."
                } else {
                    "No portable-profile key is enrolled on this device."
                },
                if (enrolled) Primary else Secondary,
            )
            Body(
                "The bundle contains add-on settings and account tokens encrypted by the Rust core. " +
                    "Package permissions are never imported automatically.",
                Secondary,
            )
            val first = rememberInitialFocus(enrolled)
            Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                if (!enrolled) {
                    Button(
                        onClick = {
                            scope.launch {
                                val result = onIo {
                                    enrollment.saveRecoveryKey(generateProfileKey())
                                    "New recovery key created and protected on this device."
                                }
                                result.onSuccess { enrolled = true }
                                report(result)
                            }
                        },
                        modifier = Modifier.focusRequester(first),
                    ) { Text("Create profile key") }
                    Button(onClick = { openKey.launch(arrayOf("text/plain", "application/octet-stream")) }) {
                        Text("Import recovery key")
                    }
                } else {
                    Button(
                        onClick = {
                            scope.launch {
                                val result = onIo {
                                    val key = requireNotNull(enrollment.loadRecoveryKey())
                                    core.exportProfile(key)
                                }
                                result.onSuccess {
                                    pendingBundle = it
                                    message = "Choose where to save the encrypted profile."
                                    error = null
                                    createBundle.launch("rvah-profile.rvah-profile")
                                }.onFailure { report(Result.failure(it)) }
                            }
                        },
                        modifier = Modifier.focusRequester(first),
                    ) { Text("Export profile") }
                    Button(onClick = { openBundle.launch(arrayOf("application/octet-stream", "*/*")) }) {
                        Text("Import profile")
                    }
                }
            }
            if (enrolled) {
                Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                    Button(onClick = {
                        scope.launch {
                            val result = onIo {
                                requireNotNull(enrollment.loadRecoveryKey())
                            }
                            result.onSuccess {
                                pendingKey = it
                                message = "Choose where to save the recovery key."
                                error = null
                                createKey.launch("rvah-recovery-key.txt")
                            }.onFailure { report(Result.failure(it)) }
                        }
                    }) { Text("Export recovery key") }
                    Button(onClick = {
                        if (confirmForget) {
                            report(runCatching {
                                enrollment.forget()
                                enrolled = false
                                confirmForget = false
                                "Local recovery key forgotten. Existing encrypted bundles were not deleted."
                            })
                        } else {
                            confirmForget = true
                            message = "Press Forget local key again to confirm. Export it first if this is the only copy."
                            error = null
                        }
                    }) { Text(if (confirmForget) "Confirm forget local key" else "Forget local key") }
                }
            }
            message?.let { Body(it, Secondary) }
            error?.let { Body(it, ErrorText) }
            Body(
                "For a second device: export the encrypted profile and recovery key, transfer them separately, " +
                    "then import the key before the profile. Web pairing automates the same separation.",
                Secondary,
            )
        }
    }
}

private const val MAX_PROFILE_BUNDLE_BYTES = 512 * 1024

/** Bound untrusted document-picker input before it can consume device memory. */
internal fun InputStream.readBoundedProfile(): ByteArray {
    val output = ByteArrayOutputStream(MAX_PROFILE_BUNDLE_BYTES)
    val buffer = ByteArray(8 * 1024)
    var total = 0
    while (true) {
        val count = read(buffer)
        if (count < 0) break
        total += count
        require(total <= MAX_PROFILE_BUNDLE_BYTES) { "profile bundle exceeds the 512 KiB limit" }
        output.write(buffer, 0, count)
    }
    return output.toByteArray()
}
