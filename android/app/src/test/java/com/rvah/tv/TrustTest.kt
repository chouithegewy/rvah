package com.rvah.tv

import java.io.File
import java.nio.file.Files
import org.junit.Assert.assertEquals
import org.junit.Test

class TrustTest {
    private val key = "0123456789abcdef".repeat(4)

    @Test fun trustedKeysKeepOnlyWellFormedHex() {
        val other = "f".repeat(64)
        assertEquals(
            listOf(key, other),
            parseTrustedKeys(" ${key.uppercase()}, $other\n$key, nothex, ${"a".repeat(63)}"),
        )
        assertEquals(emptyList<String>(), parseTrustedKeys(""))
    }

    @Test fun fingerprintIsGroupedPrefix() {
        assertEquals("0123 4567 89ab cdef", keyFingerprint(key))
    }

    @Test fun packagesNeedADetachedSignature() {
        val dir = Files.createTempDirectory("rvah").toFile()
        File(dir, "signed.rvah").writeText("x")
        File(dir, "signed.rvah.sig").writeText("00")
        File(dir, "unsigned.rvah").writeText("x")
        File(dir, "notes.txt").writeText("x")
        assertEquals(listOf("signed.rvah"), findPackages(dir).map { it.name })
        assertEquals(emptyList<File>(), findPackages(null))
    }

    @Test fun bytesAreHumanReadable() {
        assertEquals("64 KiB", formatBytes(65_536))
        assertEquals("1 MiB", formatBytes(1_048_576))
        assertEquals("12 bytes", formatBytes(12))
    }
}
