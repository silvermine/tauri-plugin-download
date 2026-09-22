package org.silvermine.downloadmanager

import org.junit.Assert.assertEquals
import org.junit.Test
import java.io.IOException

class CommandErrorTest {
   @Test
   fun `command codes depend on exception types not messages`() {
      for ((error, code) in listOf(
         DownloadException.NotFound("/tmp/file") to "download not found",
         IllegalArgumentException("timeout") to "invalid input",
         IllegalStateException("HTTP 404") to "invalid state",
         IOException("invalid path") to "file",
         DownloadException.Store(IOException("permission denied")) to "store",
         Exception("timeout") to "unknown",
      )) {
         assertEquals(code, commandErrorCode(error))
      }
   }
}
