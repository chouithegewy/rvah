package com.rvah.tv

import org.junit.Assert.assertEquals
import org.junit.Assert.assertThrows
import org.junit.Test
import java.io.ByteArrayInputStream

class ProfileEnrollmentStoreTest {
    @Test
    fun recoveryKeyIsNormalized() {
        assertEquals("ab".repeat(32), normalizeRecoveryKey("  " + "AB".repeat(32) + "\n"))
    }

    @Test
    fun recoveryKeyMustBeExactHex() {
        assertThrows(IllegalArgumentException::class.java) { normalizeRecoveryKey("ab".repeat(31)) }
        assertThrows(IllegalArgumentException::class.java) { normalizeRecoveryKey("zz".repeat(32)) }
    }

    @Test
    fun profileDocumentReadIsBoundedBeforeRustImport() {
        val valid = ByteArray(512 * 1024) { 7 }
        assertEquals(valid.size, ByteArrayInputStream(valid).readBoundedProfile().size)
        assertThrows(IllegalArgumentException::class.java) {
            ByteArrayInputStream(ByteArray(valid.size + 1)).readBoundedProfile()
        }
    }
}
