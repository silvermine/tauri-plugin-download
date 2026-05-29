/**
 * Sanity checks to test the bridge between TypeScript and the Tauri commands.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { mockIPC, clearMocks } from '@tauri-apps/api/mocks';
import { list, get } from './index';
import {
   DownloadStatus,
   DownloadAction,
   hasAction,
   hasAnyAction,
} from './types';
import { attachDownload, wrapListenerWithAutoUnlisten } from './actions';

const { eventListenMock } = vi.hoisted(() => {
   return {
      eventListenMock: vi.fn(),
   };
});

vi.mock('@tauri-apps/api/event', () => {
   return {
      listen: eventListenMock,
   };
});

let lastCmd = '',
    lastArgs: Record<string, unknown> = {};

const IDLE_STATE = {
   url: 'https://example.com/file.zip',
   path: '/tmp/file.zip',
   progress: 0,
   transferredBytes: 0,
   totalBytes: null,
   status: DownloadStatus.Idle,
};

const IN_PROGRESS_STATE = {
   url: 'https://example.com/file.zip',
   path: '/tmp/file.zip',
   progress: 42,
   transferredBytes: 420,
   totalBytes: 1000,
   status: DownloadStatus.InProgress,
};

const PAUSED_STATE = {
   url: 'https://example.com/file.zip',
   path: '/tmp/file.zip',
   progress: 42,
   transferredBytes: 420,
   totalBytes: 1000,
   status: DownloadStatus.Paused,
};

const ACTION_RESPONSE_BASE = {
   isExpectedStatus: true,
};

beforeEach(() => {
   eventListenMock.mockReset();

   mockIPC((cmd: string, args: unknown) => {
      lastCmd = cmd;
      lastArgs = args as Record<string, unknown>;

      if (cmd === 'plugin:download|list') {
         return [ IDLE_STATE ];
      }
      if (cmd === 'plugin:download|get') {
         const path = (args as { path: string }).path;

         if (path === '/tmp/file.zip') {
            return IDLE_STATE;
         }
         return {
            url: '',
            path,
            progress: 0,
            transferredBytes: 0,
            totalBytes: null,
            status: DownloadStatus.Pending,
         };
      }
      if (cmd === 'plugin:download|create') {
         return {
            ...ACTION_RESPONSE_BASE,
            expectedStatus: DownloadStatus.Idle,
            download: IDLE_STATE,
         };
      }
      if (cmd === 'plugin:download|start') {
         return {
            ...ACTION_RESPONSE_BASE,
            expectedStatus: DownloadStatus.InProgress,
            download: IN_PROGRESS_STATE,
         };
      }
      if (cmd === 'plugin:download|pause') {
         return {
            ...ACTION_RESPONSE_BASE,
            expectedStatus: DownloadStatus.Paused,
            download: PAUSED_STATE,
         };
      }
      if (cmd === 'plugin:download|resume') {
         return {
            ...ACTION_RESPONSE_BASE,
            expectedStatus: DownloadStatus.InProgress,
            download: IN_PROGRESS_STATE,
         };
      }
      if (cmd === 'plugin:download|cancel') {
         return {
            ...ACTION_RESPONSE_BASE,
            expectedStatus: DownloadStatus.Canceled,
            download: { ...IDLE_STATE, status: DownloadStatus.Canceled },
         };
      }
      if (cmd === 'plugin:download|is_native') {
         return false;
      }
      return undefined;
   });
});

afterEach(() => {
   eventListenMock.mockReset();

   return clearMocks();
});

describe('list', () => {
   it('invokes the correct command and returns downloads with actions attached', async () => {
      const downloads = await list();

      expect(lastCmd).toBe('plugin:download|list');
      expect(downloads).toHaveLength(1);
      expect(downloads[0].status).toBe(DownloadStatus.Idle);
      expect(hasAction(downloads[0], DownloadAction.Start)).toBe(true);
   });
});

describe('get', () => {
   it('invokes the correct command and returns an Idle download with actions attached', async () => {
      const download = await get('/tmp/file.zip');

      expect(lastCmd).toBe('plugin:download|get');
      expect(lastArgs.path).toBe('/tmp/file.zip');
      expect(download.status).toBe(DownloadStatus.Idle);
      expect(hasAction(download, DownloadAction.Start)).toBe(true);
      expect(hasAction(download, DownloadAction.Cancel)).toBe(true);
      expect(hasAction(download, DownloadAction.Resume)).toBe(false);
   });

   it('returns a Pending download for unknown path', async () => {
      const download = await get('/tmp/unknown.zip');

      expect(download.status).toBe(DownloadStatus.Pending);
      expect(hasAction(download, DownloadAction.Create)).toBe(true);
      expect(hasAnyAction(download)).toBe(true);
   });
});

describe('download actions', () => {
   it('create — sends path and url, returns Idle download', async () => {
      const pending = await get('/tmp/unknown.zip');

      if (!hasAction(pending, DownloadAction.Create)) {
         throw new Error('expected create action');
      }
      const response = await pending.create('https://example.com/file.zip');

      expect(lastCmd).toBe('plugin:download|create');
      expect(lastArgs.path).toBe('/tmp/unknown.zip');
      expect(lastArgs.url).toBe('https://example.com/file.zip');
      expect(response.isExpectedStatus).toBe(true);
      expect(response.download.status).toBe(DownloadStatus.Idle);
   });

   it('start — sends path, returns InProgress download', async () => {
      const download = await get('/tmp/file.zip');

      if (!hasAction(download, DownloadAction.Start)) {
         throw new Error('expected start action');
      }
      const response = await download.start();

      expect(lastCmd).toBe('plugin:download|start');
      expect(lastArgs.path).toBe('/tmp/file.zip');
      expect(response.isExpectedStatus).toBe(true);
      expect(response.download.status).toBe(DownloadStatus.InProgress);
   });

   it('pause — sends path, returns Paused download', async () => {
      const inProgress = attachDownload(IN_PROGRESS_STATE);

      expect(hasAction(inProgress, DownloadAction.Pause)).toBe(true);
      const response = await inProgress.pause();

      expect(lastCmd).toBe('plugin:download|pause');
      expect(lastArgs.path).toBe('/tmp/file.zip');
      expect(response.isExpectedStatus).toBe(true);
      expect(response.download.status).toBe(DownloadStatus.Paused);
   });

   it('resume — sends path, returns InProgress download', async () => {
      const paused = attachDownload(PAUSED_STATE);

      expect(hasAction(paused, DownloadAction.Resume)).toBe(true);
      const response = await paused.resume();

      expect(lastCmd).toBe('plugin:download|resume');
      expect(lastArgs.path).toBe('/tmp/file.zip');
      expect(response.isExpectedStatus).toBe(true);
      expect(response.download.status).toBe(DownloadStatus.InProgress);
   });

   it('cancel — sends path, returns Canceled download', async () => {
      const download = await get('/tmp/file.zip');

      if (!hasAction(download, DownloadAction.Cancel)) {
         throw new Error('expected cancel action');
      }
      const response = await download.cancel();

      expect(lastCmd).toBe('plugin:download|cancel');
      expect(lastArgs.path).toBe('/tmp/file.zip');
      expect(response.isExpectedStatus).toBe(true);
      expect(response.download.status).toBe(DownloadStatus.Canceled);
   });

   it('handles errors thrown by the backend', async () => {
      mockIPC(() => { throw new Error('download error'); });

      const download = await get('/tmp/file.zip').catch(() => {
         return attachDownload(IDLE_STATE);
      });

      if (!hasAction(download, DownloadAction.Start)) {
         throw new Error('expected start action');
      }
      await expect(download.start()).rejects.toThrow('download error');
   });

});

describe('state machine — action availability', () => {
   it('Pending: only create and listen are available', () => {
      const download = attachDownload({
         url: '',
         path: '/tmp/file.zip',
         progress: 0,
         transferredBytes: 0,
         totalBytes: null,
         status: DownloadStatus.Pending,
      });

      expect(hasAction(download, DownloadAction.Create)).toBe(true);
      expect(hasAction(download, DownloadAction.Listen)).toBe(true);
      expect(hasAction(download, DownloadAction.Start)).toBe(false);
      expect(hasAction(download, DownloadAction.Pause)).toBe(false);
      expect(hasAction(download, DownloadAction.Resume)).toBe(false);
      expect(hasAction(download, DownloadAction.Cancel)).toBe(false);
   });

   it('Idle: start, cancel, and listen are available', () => {
      const download = attachDownload(IDLE_STATE);

      expect(hasAction(download, DownloadAction.Start)).toBe(true);
      expect(hasAction(download, DownloadAction.Cancel)).toBe(true);
      expect(hasAction(download, DownloadAction.Listen)).toBe(true);
      expect(hasAction(download, DownloadAction.Create)).toBe(false);
      expect(hasAction(download, DownloadAction.Pause)).toBe(false);
      expect(hasAction(download, DownloadAction.Resume)).toBe(false);
   });

   it('InProgress: pause, cancel, and listen are available', () => {
      const download = attachDownload(IN_PROGRESS_STATE);

      expect(hasAction(download, DownloadAction.Pause)).toBe(true);
      expect(hasAction(download, DownloadAction.Cancel)).toBe(true);
      expect(hasAction(download, DownloadAction.Listen)).toBe(true);
      expect(hasAction(download, DownloadAction.Start)).toBe(false);
      expect(hasAction(download, DownloadAction.Resume)).toBe(false);
   });

   it('Paused: resume, cancel, and listen are available', () => {
      const download = attachDownload(PAUSED_STATE);

      expect(hasAction(download, DownloadAction.Resume)).toBe(true);
      expect(hasAction(download, DownloadAction.Cancel)).toBe(true);
      expect(hasAction(download, DownloadAction.Listen)).toBe(true);
      expect(hasAction(download, DownloadAction.Start)).toBe(false);
      expect(hasAction(download, DownloadAction.Pause)).toBe(false);
   });

   it('Completed: no actions available', () => {
      const download = attachDownload({
         ...IDLE_STATE,
         status: DownloadStatus.Completed,
      });

      expect(hasAnyAction(download)).toBe(false);
      expect(hasAction(download, DownloadAction.Cancel)).toBe(false);
   });

   it('Canceled: no actions available', () => {
      const download = attachDownload({
         ...IDLE_STATE,
         status: DownloadStatus.Canceled,
      });

      expect(hasAnyAction(download)).toBe(false);
      expect(hasAction(download, DownloadAction.Cancel)).toBe(false);
   });

   it('Unknown: only listen is available', () => {
      const download = attachDownload({
         ...IDLE_STATE,
         status: DownloadStatus.Unknown,
      });

      expect(hasAction(download, DownloadAction.Listen)).toBe(true);
      expect(hasAnyAction(download)).toBe(true);
      expect(hasAction(download, DownloadAction.Cancel)).toBe(false);
   });

   it('attaches only the allowed methods as callable functions', () => {
      const idle = attachDownload(IDLE_STATE);

      expect(typeof idle.start).toBe('function');
      expect(typeof idle.cancel).toBe('function');
      expect(typeof (idle as unknown as Record<string, unknown>).pause).toBe('undefined');
   });

   it('preserves all state fields on the returned object', () => {
      const download = attachDownload(IN_PROGRESS_STATE);

      expect(download.url).toBe('https://example.com/file.zip');
      expect(download.path).toBe('/tmp/file.zip');
      expect(download.progress).toBe(42);
      expect(download.transferredBytes).toBe(420);
      expect(download.totalBytes).toBe(1000);
      expect(download.status).toBe(DownloadStatus.InProgress);
   });
});

describe('wrapListenerWithAutoUnlisten', () => {
   it('calls the listener and does not unlisten for non-terminal states', () => {
      const listener = vi.fn();

      const unlisten = vi.fn();

      const wrappedListener = wrapListenerWithAutoUnlisten(listener, unlisten);

      wrappedListener(attachDownload({
         ...IDLE_STATE,
         status: DownloadStatus.InProgress,
      }));
      wrappedListener(attachDownload({
         ...IDLE_STATE,
         status: DownloadStatus.Paused,
      }));

      expect(listener).toHaveBeenCalledTimes(2);
      expect(unlisten).toHaveBeenCalledTimes(0);
   });

   it('calls unlisten on Completed', () => {
      const listener = vi.fn();

      const unlisten = vi.fn();

      const wrappedListener = wrapListenerWithAutoUnlisten(listener, unlisten);

      wrappedListener(attachDownload({
         ...IDLE_STATE,
         status: DownloadStatus.Completed,
      }));

      expect(listener).toHaveBeenCalledTimes(1);
      expect(unlisten).toHaveBeenCalledTimes(1);
   });

   it('calls unlisten on Canceled', () => {
      const listener = vi.fn();

      const unlisten = vi.fn();

      const wrappedListener = wrapListenerWithAutoUnlisten(listener, unlisten);

      wrappedListener(attachDownload({
         ...IDLE_STATE,
         status: DownloadStatus.Canceled,
      }));

      expect(listener).toHaveBeenCalledTimes(1);
      expect(unlisten).toHaveBeenCalledTimes(1);
   });

   it('calls unlisten on Completed when the callback throws', () => {
      const unlisten = vi.fn();

      const wrappedListener = wrapListenerWithAutoUnlisten(() => {
         throw new Error('listener error');
      }, unlisten);

      let thrownError: unknown;

      try {
         wrappedListener(attachDownload({
            ...IDLE_STATE,
            status: DownloadStatus.Completed,
         }));
      } catch(error) {
         thrownError = error;
      }

      expect(thrownError).toBeInstanceOf(Error);
      expect((thrownError as Error).message).toBe('listener error');
      expect(unlisten).toHaveBeenCalledTimes(1);
   });
});
