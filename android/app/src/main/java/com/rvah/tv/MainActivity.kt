package com.rvah.tv

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.lifecycle.lifecycleScope
import uniffi.rvah_android.AndroidCore

class MainActivity : ComponentActivity() {
    private lateinit var owner: RvahPlayerOwner

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        owner = RvahPlayerOwner(this, lifecycleScope)
        // Opening the core touches SQLite and compiles every installed add-on;
        // RvahTvApp calls this on Dispatchers.IO, never on the main thread.
        val openCore = {
            AndroidCore.open(filesDir.absolutePath, AndroidKeystoreSecretStore(applicationContext))
        }
        setContent {
            RvahTvApp(
                openCore = openCore,
                owner = owner,
                packagesDir = getExternalFilesDir("addons"),
                trustedKeys = parseTrustedKeys(BuildConfig.TRUSTED_KEYS),
            )
        }
    }

    override fun onDestroy() {
        owner.release()
        super.onDestroy()
    }
}
