package com.rvah.tv

import uniffi.rvah_android.AndroidCoreException

/** The Rust core's reason, without UniFFI's `reason=` debug framing. */
fun Throwable.userMessage(): String = when (this) {
    is AndroidCoreException.Storage -> reason
    is AndroidCoreException.Addon -> reason
    is AndroidCoreException.Playback -> reason
    else -> message ?: javaClass.simpleName
}
