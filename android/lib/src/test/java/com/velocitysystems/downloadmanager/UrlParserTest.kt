package com.velocitysystems.downloadmanager

import org.junit.Assert.assertEquals
import org.junit.Assert.assertThrows
import org.junit.Test

class UrlParserTest {

    // -- parsePath tests --

    @Test
    fun `valid absolute path`() {
        assertEquals("/downloads/file.mp4", parsePath("/downloads/file.mp4"))
        assertEquals("/file.txt", parsePath("/file.txt"))
    }

    @Test
    fun `valid file URL`() {
        assertEquals("/downloads/file.mp4", parsePath("file:///downloads/file.mp4"))
        assertEquals("/file.txt", parsePath("file:///file.txt"))
    }

    @Test
    fun `empty path throws`() {
        assertThrows(IllegalArgumentException::class.java) {
            parsePath("")
        }
    }

    @Test
    fun `relative path throws`() {
        assertThrows(IllegalArgumentException::class.java) {
            parsePath("relative/path.txt")
        }
        assertThrows(IllegalArgumentException::class.java) {
            parsePath("file.txt")
        }
    }

    @Test
    fun `path without filename throws`() {
        assertThrows(IllegalArgumentException::class.java) {
            parsePath("/")
        }
    }

    // -- parseUrl tests --

    @Test
    fun `valid URLs`() {
        assertEquals("https://example.com/file.mp4", parseUrl("https://example.com/file.mp4"))
        assertEquals("http://example.com/file.mp4", parseUrl("http://example.com/file.mp4"))
        assertEquals("https://example.com:8080/file.mp4", parseUrl("https://example.com:8080/file.mp4"))
        assertEquals("https://example.com/file.mp4?token=abc", parseUrl("https://example.com/file.mp4?token=abc"))
    }

    @Test
    fun `empty URL throws`() {
        assertThrows(IllegalArgumentException::class.java) {
            parseUrl("")
        }
    }

    @Test
    fun `invalid scheme throws`() {
        assertThrows(IllegalArgumentException::class.java) {
            parseUrl("ftp://example.com/file.mp4")
        }
        assertThrows(IllegalArgumentException::class.java) {
            parseUrl("file:///path/to/file.mp4")
        }
    }

    @Test
    fun `missing host throws`() {
        assertThrows(IllegalArgumentException::class.java) {
            parseUrl("https://:8080/file.mp4")
        }
    }

    @Test
    fun `invalid URL format throws`() {
        assertThrows(IllegalArgumentException::class.java) {
            parseUrl("not a valid url")
        }
    }
}
