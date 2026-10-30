package com.rvah.tv

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class WebPolicyTest {
    @Test fun mainFrameIsDocument() = assertEquals("document", requestType("https://a.example/x.php", true, null))

    @Test fun typesByExtensionThenAccept() {
        assertEquals("script", requestType("https://a.example/p.js?v=2", false, null))
        assertEquals("stylesheet", requestType("https://a.example/s.css", false, null))
        assertEquals("image", requestType("https://a.example/logo.SVG", false, null))
        assertEquals("media", requestType("https://a.example/seg/1.ts", false, null))
        assertEquals("font", requestType("https://a.example/f.woff2", false, null))
        assertEquals("sub_frame", requestType("https://b.example/embed.php", false, "text/html,application/xhtml+xml"))
        assertEquals("image", requestType("https://b.example/i", false, "image/avif,image/webp"))
        assertEquals("stylesheet", requestType("https://b.example/c", false, "text/css,*/*;q=0.1"))
        assertEquals("xmlhttprequest", requestType("https://b.example/api", false, "application/json"))
        assertEquals("other", requestType("https://b.example/i", false, null))
    }

    @Test fun neverBlockedMatchesHostAndSubdomains() {
        val list = listOf("cdn.example.net")
        assertTrue(neverBlocked("cdn.example.net", list))
        assertTrue(neverBlocked("p16.CDN.example.net", list))
        assertFalse(neverBlocked("evilcdn.example.net", list))
        assertFalse(neverBlocked("example.net", list))
    }

    @Test fun onlyMainFrameNavigationIsHeldToTheSourceHost() {
        val src = "https://embed.example/ch.php"
        assertTrue(navigationAllowed(src, "https://embed.example/other", true))
        assertFalse(navigationAllowed(src, "https://ads.example/pop", true))
        assertFalse(navigationAllowed(src, "intent://pop#Intent;end", true))
        assertTrue(navigationAllowed(src, "https://player.example/frame", false))
    }

    @Test fun listsGoStaleAfterSevenDays() {
        val day = 24 * 3600 * 1000L
        assertTrue(listsStale(0L, 10 * day))
        assertFalse(listsStale(day, 7 * day))
        assertTrue(listsStale(day, 8 * day))
    }
}
