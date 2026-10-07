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
        val core = AndroidCore.open(filesDir.absolutePath, AndroidKeystoreSecretStore(this))
        owner = RvahPlayerOwner(this, lifecycleScope, core)
        setContent { RvahTvApp(core, owner) }
    }

    override fun onDestroy() {
        owner.release()
        super.onDestroy()
    }
}
