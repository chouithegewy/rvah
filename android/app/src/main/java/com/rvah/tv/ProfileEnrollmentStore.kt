package com.rvah.tv

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/**
 * Device-local protection for the portable-profile recovery key.
 *
 * The profile bundle is already encrypted by the Rust core. The recovery key
 * is wrapped separately by a non-exportable Android Keystore key so neither is
 * persisted as plaintext by the app.
 */
internal class ProfileEnrollmentStore(context: Context) {
    private val preferences = context.getSharedPreferences(PREFERENCES, Context.MODE_PRIVATE)

    private fun wrappingKey(): SecretKey {
        val store = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        (store.getKey(KEY_ALIAS, null) as? SecretKey)?.let { return it }
        return KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore").run {
            init(
                KeyGenParameterSpec.Builder(
                    KEY_ALIAS,
                    KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT,
                )
                    .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                    .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                    .setKeySize(256)
                    .build(),
            )
            generateKey()
        }
    }

    fun saveRecoveryKey(value: String) {
        val recoveryKey = normalizeRecoveryKey(value)
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.ENCRYPT_MODE, wrappingKey())
        cipher.updateAAD(AAD)
        val blob = cipher.iv + cipher.doFinal(recoveryKey.toByteArray(Charsets.US_ASCII))
        check(preferences.edit().putString(RECOVERY_KEY, Base64.encodeToString(blob, Base64.NO_WRAP)).commit()) {
            "could not store the protected recovery key"
        }
    }

    fun loadRecoveryKey(): String? {
        val encoded = preferences.getString(RECOVERY_KEY, null) ?: return null
        val blob = Base64.decode(encoded, Base64.NO_WRAP)
        require(blob.size > IV_BYTES) { "protected recovery key is truncated" }
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.DECRYPT_MODE, wrappingKey(), GCMParameterSpec(128, blob.copyOfRange(0, IV_BYTES)))
        cipher.updateAAD(AAD)
        return normalizeRecoveryKey(
            cipher.doFinal(blob.copyOfRange(IV_BYTES, blob.size)).toString(Charsets.US_ASCII),
        )
    }

    /** Remove both the ciphertext and its device-only wrapping key. */
    fun forget() {
        check(preferences.edit().clear().commit()) { "could not clear the protected recovery key" }
        KeyStore.getInstance("AndroidKeyStore").apply {
            load(null)
            if (containsAlias(KEY_ALIAS)) deleteEntry(KEY_ALIAS)
        }
    }

    private companion object {
        const val PREFERENCES = "rvah_profile_enrollment"
        const val RECOVERY_KEY = "recovery_key"
        const val KEY_ALIAS = "rvah.profile.enrollment.aes-gcm"
        const val IV_BYTES = 12
        val AAD = "rvah-profile-enrollment-v1".toByteArray(Charsets.US_ASCII)
    }
}

internal fun normalizeRecoveryKey(value: String): String {
    val normalized = value.trim().lowercase()
    require(Regex("[0-9a-f]{64}").matches(normalized)) {
        "a recovery key must contain exactly 64 hexadecimal characters"
    }
    return normalized
}
