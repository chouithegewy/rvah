package com.rvah.tv

import org.junit.Assert.assertEquals
import org.junit.Test

class DevSecretsTest {
    @Test fun gradleEncodingParses() {
        assertEquals(
            mapOf("TMDB_API_KEY" to "abc123", "MDBLIST_API_KEY" to "m.d-b_1"),
            parseDevEnv("TMDB_API_KEY=abc123;MDBLIST_API_KEY=m.d-b_1"),
        )
        assertEquals(emptyMap<String, String>(), parseDevEnv(""))
        assertEquals(mapOf("A" to "1"), parseDevEnv("A=1;B=;junk"))
    }

    @Test fun theReadAccessTokenWinsOverTheApiKey() {
        val seeds = devSecrets(
            mapOf("TMDB_API_KEY" to "key", "TMDB_READ_ACCESS_TOKEN" to "eyJtoken", "MDBLIST_API_KEY" to "mdb"),
        )
        assertEquals(
            listOf(
                SeedSecret(TMDB_ADDON, "tmdb_key", "eyJtoken"),
                SeedSecret(TMDB_ADDON, "mdblist_key", "mdb"),
            ),
            seeds,
        )
        assertEquals(listOf(SeedSecret(TMDB_ADDON, "tmdb_key", "key")), devSecrets(mapOf("TMDB_API_KEY" to "key")))
        assertEquals(emptyList<SeedSecret>(), devSecrets(mapOf("OTHER" to "x")))
        assertEquals(
            listOf(SeedSecret(PROWLARR_ADDON, "api_key", "pk")),
            devSecrets(mapOf("PROWLARR_API_KEY" to "pk", "PROWLARR_URL" to "https://x")),
        )
    }
}
