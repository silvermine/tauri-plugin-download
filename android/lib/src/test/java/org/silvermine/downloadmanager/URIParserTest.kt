package org.silvermine.downloadmanager

import org.junit.Assert.assertEquals
import org.junit.Assert.assertThrows
import org.junit.Test

class URIParserTest {

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
   fun `path traversal is resolved`() {
      assertEquals("/downloads/file.txt", parsePath("/downloads/subdir/../file.txt"))
      assertEquals("/file.txt", parsePath("/a/b/../../file.txt"))
   }

   @Test
   fun `path without filename throws`() {
      assertThrows(IllegalArgumentException::class.java) {
         parsePath("/")
      }
   }

   // -- parseURI tests --

   @Test
   fun `valid URLs`() {
      assertEquals("https://example.com/file.mp4", parseURI("https://example.com/file.mp4"))
      assertEquals("http://example.com/file.mp4", parseURI("http://example.com/file.mp4"))
      assertEquals("https://example.com:8080/file.mp4", parseURI("https://example.com:8080/file.mp4"))
      assertEquals("https://example.com/file.mp4?token=abc", parseURI("https://example.com/file.mp4?token=abc"))
   }

   @Test
   fun `empty URL throws`() {
      assertThrows(IllegalArgumentException::class.java) {
         parseURI("")
      }
   }

   @Test
   fun `invalid scheme throws`() {
      assertThrows(IllegalArgumentException::class.java) {
         parseURI("ftp://example.com/file.mp4")
      }
      assertThrows(IllegalArgumentException::class.java) {
         parseURI("file:///path/to/file.mp4")
      }
   }

   @Test
   fun `missing host throws`() {
      assertThrows(IllegalArgumentException::class.java) {
         parseURI("https://:8080/file.mp4")
      }
   }

   @Test
   fun `URL with credentials throws`() {
      assertThrows(IllegalArgumentException::class.java) {
         parseURI("https://user:pass@example.com/file.mp4")
      }
      assertThrows(IllegalArgumentException::class.java) {
         parseURI("https://user@example.com/file.mp4")
      }
   }

   @Test
   fun `invalid URL format throws`() {
      assertThrows(IllegalArgumentException::class.java) {
         parseURI("not a valid url")
      }
      assertThrows(IllegalArgumentException::class.java) {
         parseURI("/not a valid url")
      }
   }
}
