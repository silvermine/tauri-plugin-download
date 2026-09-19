package org.silvermine.downloadmanager

import org.junit.Assert.assertEquals
import org.junit.Test

class DownloadExceptionTest {

   @Test
   fun `not found message matches the other platforms`() {
      assertEquals("Not Found: /tmp/file.mp4", DownloadException.NotFound("/tmp/file.mp4").message)
   }
}
