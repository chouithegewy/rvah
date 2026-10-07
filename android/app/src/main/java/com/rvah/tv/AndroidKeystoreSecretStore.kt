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
import uniffi.rvah_android.AndroidSecretStore
import uniffi.rvah_android.SecretRead

class AndroidKeystoreSecretStore(context: Context) : AndroidSecretStore {
    private val preferences = context.getSharedPreferences("rvah_keystore_secrets", Context.MODE_PRIVATE)
    private val alias = "rvah.addon.secrets.aes-gcm"

    private fun secretKey(): SecretKey {
        val store = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        (store.getKey(alias, null) as? SecretKey)?.let { return it }
        return KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore").run {
            init(
                KeyGenParameterSpec.Builder(
                    alias,
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

    override fun put(key: String, value: ByteArray): String? = runCatching {
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.ENCRYPT_MODE, secretKey())
        val blob = cipher.iv + cipher.doFinal(value)
        check(preferences.edit().putString(key, Base64.encodeToString(blob, Base64.NO_WRAP)).commit())
    }.exceptionOrNull()?.message

    override fun get(key: String): SecretRead {
        val encoded = preferences.getString(key, null) ?: return SecretRead(null, null)
        return runCatching {
            val blob = Base64.decode(encoded, Base64.NO_WRAP)
            require(blob.size > 12) { "encrypted secret is truncated" }
            val cipher = Cipher.getInstance("AES/GCM/NoPadding")
            cipher.init(Cipher.DECRYPT_MODE, secretKey(), GCMParameterSpec(128, blob.copyOfRange(0, 12)))
            SecretRead(cipher.doFinal(blob.copyOfRange(12, blob.size)), null)
        }.getOrElse { SecretRead(null, it.message ?: "keystore decryption failed") }
    }

    override fun delete(key: String): String? = runCatching {
        check(preferences.edit().remove(key).commit())
    }.exceptionOrNull()?.message

    override fun list(prefix: String): List<String> = preferences.all.keys.filter { it.startsWith(prefix) }
}
