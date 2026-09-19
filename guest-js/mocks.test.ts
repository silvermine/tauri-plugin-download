import { afterEach, describe, expect, it, vi } from 'vitest';
import { invoke } from '@tauri-apps/api/core';
import { get, list } from './index';
import { clearDownloadMocks, createMockDownloadState, mockDownloadPlugin } from './mocks';
import {
   allowedActions,
   DownloadAction,
   type DownloadActionResponse,
   DownloadStatus,
   expectedStatusesForAction,
   hasAction,
} from './types';

afterEach(() => {
   clearDownloadMocks();
});

const ACTIONS = [
   DownloadAction.Create,
   DownloadAction.Start,
   DownloadAction.Resume,
   DownloadAction.Pause,
   DownloadAction.Cancel,
] as const;

const PATH_ACTIONS = [
   DownloadAction.Start,
   DownloadAction.Resume,
   DownloadAction.Pause,
   DownloadAction.Cancel,
] as const;

const REMOVED_STATUSES = [
   DownloadStatus.Canceled,
   DownloadStatus.Completed,
] as const;

const STORED_STATUSES = [
   DownloadStatus.Idle,
   DownloadStatus.InProgress,
   DownloadStatus.Paused,
] as const;

const UNSTORED_STATUSES = [
   DownloadStatus.Pending,
   DownloadStatus.Canceled,
   DownloadStatus.Completed,
] as const;

const ACTION_STATUS_CASES = ACTIONS.flatMap((action) => {
   return STORED_STATUSES.map((status) => {
      return [ action, status ] as const;
   });
});

function getExpectedStatus(action: Exclude<DownloadAction, DownloadAction.Listen>): DownloadStatus {
   const expectedStatus = expectedStatusesForAction[action][0];

   if (!expectedStatus) {
      throw new Error(`Action ${action} must define an expected status for test coverage`);
   }

   return expectedStatus;
}

async function invokeAction(action: Exclude<DownloadAction, DownloadAction.Listen>, path: string): Promise<DownloadActionResponse> {
   const args = action === DownloadAction.Create ?
      { path, url: 'https://example.com/recreated.zip' } :
      { path };

   return invoke<DownloadActionResponse>(`plugin:download|${action}`, args);
}

describe('mockDownloadPlugin', () => {
   it('normalizes completed progress while preserving byte counts', () => {
      const state = createMockDownloadState(DownloadStatus.Completed, {
         receivedBytes: 30,
         totalBytes: 100,
      });

      expect(state).toEqual(expect.objectContaining({
         receivedBytes: 30,
         totalBytes: 100,
         progress: 100,
      }));
   });

   it('clamps derived progress to 100 percent', () => {
      const { progress } = createMockDownloadState(DownloadStatus.InProgress, {
         receivedBytes: 150,
         totalBytes: 100,
      });

      expect(progress).toBe(100);
   });

   it('seeds downloads for list and records invocations', async () => {
      const controller = mockDownloadPlugin({
         downloads: [
            createMockDownloadState(DownloadStatus.Idle, {
               path: '/tmp/seeded.zip',
               progress: 12,
            }),
         ],
      });

      const downloads = await list();

      expect(downloads).toHaveLength(1);
      expect(downloads[0].path).toBe('/tmp/seeded.zip');
      expect(downloads[0].status).toBe(DownloadStatus.Idle);
      expect(hasAction(downloads[0], DownloadAction.Start)).toBe(true);
      expect(controller.getLastInvocation()).toEqual({
         cmd: 'plugin:download|list',
         args: {},
      });
   });

   it('transitions Pending to Idle on create and updates stored state', async () => {
      const controller = mockDownloadPlugin();

      const download = await get('/tmp/new.zip');

      expect(download.status).toBe(DownloadStatus.Pending);
      expect(hasAction(download, DownloadAction.Create)).toBe(true);
      if (!hasAction(download, DownloadAction.Create)) {
         throw new Error('expected create action');
      }

      const response = await download.create('https://example.com/new.zip', { allowMetered: false });

      expect(response.isExpectedStatus).toBe(true);
      expect(response.download.status).toBe(DownloadStatus.Idle);
      expect(controller.getLastInvocation()?.args.options).toEqual({ allowMetered: false });
      expect(controller.getDownload('/tmp/new.zip')).toEqual({
         url: 'https://example.com/new.zip',
         path: '/tmp/new.zip',
         options: { allowMetered: false },
         receivedBytes: 0,
         totalBytes: null,
         progress: 0,
         status: DownloadStatus.Idle,
      });
   });

   it('keeps create-time options when create is repeated', async () => {
      const path = '/tmp/existing.zip';

      const controller = mockDownloadPlugin({
         downloads: [
            createMockDownloadState(DownloadStatus.Idle, {
               path,
               options: { allowMetered: false },
            }),
         ],
      });

      const response = await invoke<DownloadActionResponse>('plugin:download|create', {
         path,
         url: 'https://example.com/recreated.zip',
         options: { allowMetered: true },
      });

      expect(response.download.options).toEqual({ allowMetered: false });
      expect(controller.getDownload(path).options).toEqual({ allowMetered: false });
   });

   it('emits mocked desktop events to download listeners', async () => {
      const controller = mockDownloadPlugin({
         downloads: [
            createMockDownloadState(DownloadStatus.Idle, {
               path: '/tmp/listener.zip',
            }),
         ],
      });

      const download = await get('/tmp/listener.zip');

      const listener = vi.fn();

      if (!hasAction(download, DownloadAction.Listen)) {
         throw new Error('expected listen action');
      }

      const unlisten = await download.listen(listener);

      await controller.emitChange(createMockDownloadState(DownloadStatus.InProgress, {
         path: '/tmp/listener.zip',
         receivedBytes: 42,
         totalBytes: 100,
      }));

      expect(listener).toHaveBeenCalledTimes(1);
      expect(listener).toHaveBeenCalledWith(expect.objectContaining({
         path: '/tmp/listener.zip',
         receivedBytes: 42,
         totalBytes: 100,
         status: DownloadStatus.InProgress,
      }));
      expect(hasAction(listener.mock.calls[0][0], DownloadAction.Pause)).toBe(true);

      unlisten();
   });

   it('resets listener state when clearing download mocks', async () => {
      const firstController = mockDownloadPlugin({
         downloads: [
            createMockDownloadState(DownloadStatus.Idle, {
               path: '/tmp/first-listener.zip',
            }),
         ],
      });

      const firstDownload = await get('/tmp/first-listener.zip');

      const firstListener = vi.fn();

      if (!hasAction(firstDownload, DownloadAction.Listen)) {
         throw new Error('expected listen action');
      }

      await firstDownload.listen(firstListener);

      await firstController.emitChange(createMockDownloadState(DownloadStatus.InProgress, {
         path: '/tmp/first-listener.zip',
         receivedBytes: 10,
         totalBytes: 100,
      }));

      expect(firstListener).toHaveBeenCalledTimes(1);

      clearDownloadMocks();

      const secondController = mockDownloadPlugin({
         downloads: [
            createMockDownloadState(DownloadStatus.Idle, {
               path: '/tmp/second-listener.zip',
            }),
         ],
      });

      const secondDownload = await get('/tmp/second-listener.zip');

      const secondListener = vi.fn();

      if (!hasAction(secondDownload, DownloadAction.Listen)) {
         throw new Error('expected listen action');
      }

      const unlisten = await secondDownload.listen(secondListener);

      await secondController.emitChange(createMockDownloadState(DownloadStatus.InProgress, {
         path: '/tmp/second-listener.zip',
         receivedBytes: 20,
         totalBytes: 100,
      }));

      expect(secondListener).toHaveBeenCalledTimes(1);
      unlisten();
   });

   it('allows command errors to be injected per action', async () => {
      const controller = mockDownloadPlugin({
         downloads: [
            createMockDownloadState(DownloadStatus.Idle, {
               path: '/tmp/error.zip',
            }),
         ],
      });

      const download = await get('/tmp/error.zip');

      controller.setCommandError('start', 'start failed');

      if (!hasAction(download, DownloadAction.Start)) {
         throw new Error('expected start action');
      }

      await expect(download.start()).rejects.toThrow('start failed');

      controller.clearCommandError('start');

      const response = await download.start();

      expect(response.isExpectedStatus).toBe(true);
      expect(response.download.status).toBe(DownloadStatus.InProgress);
   });

   it('removes a canceled download so it can be created again', async () => {
      const path = '/tmp/canceled.zip';

      const controller = mockDownloadPlugin({
         downloads: [
            createMockDownloadState(DownloadStatus.Idle, { path }),
         ],
      });

      const response = await invokeAction(DownloadAction.Cancel, path);

      expect(response.download.status).toBe(DownloadStatus.Canceled);
      expect(controller.listDownloads()).toEqual([]);
      expect((await get(path)).status).toBe(DownloadStatus.Pending);

      const recreated = await invokeAction(DownloadAction.Create, path);

      expect(recreated.isExpectedStatus).toBe(true);
      expect(recreated.download.status).toBe(DownloadStatus.Idle);
   });

   it.each(REMOVED_STATUSES)('removes the download when emitting a %s change', async (status) => {
      const path = '/tmp/terminal.zip';

      const controller = mockDownloadPlugin({
         downloads: [
            createMockDownloadState(DownloadStatus.InProgress, { path }),
         ],
      });

      const download = await get(path);

      const listener = vi.fn();

      if (!hasAction(download, DownloadAction.Listen)) {
         throw new Error('expected listen action');
      }

      const unlisten = await download.listen(listener);

      await controller.emitChange(createMockDownloadState(status, { path }));

      expect(listener).toHaveBeenCalledWith(expect.objectContaining({ path, status }));
      expect(controller.listDownloads()).toEqual([]);

      unlisten();
   });

   it.each(UNSTORED_STATUSES)('rejects seeding a %s download', (status) => {
      const download = createMockDownloadState(status, { path: '/tmp/seeded.zip' });

      expect(() => { mockDownloadPlugin({ downloads: [ download ] }); }).toThrow('native stores only hold');
   });

   it.each(UNSTORED_STATUSES)('rejects setDownload with a %s download', (status) => {
      const controller = mockDownloadPlugin();

      const download = createMockDownloadState(status, { path: '/tmp/set.zip' });

      expect(() => { controller.setDownload(download); }).toThrow('native stores only hold');
      expect(controller.listDownloads()).toEqual([]);
   });

   it('rejects emitting a pending change', async () => {
      const controller = mockDownloadPlugin();

      const download = createMockDownloadState(DownloadStatus.Pending, { path: '/tmp/emitted.zip' });

      await expect(controller.emitChange(download)).rejects.toThrow('native stores only hold');
   });

   it('creates a download for a path with no stored download', async () => {
      const path = '/tmp/created.zip';

      const controller = mockDownloadPlugin();

      const response = await invokeAction(DownloadAction.Create, path);

      expect(response.isExpectedStatus).toBe(true);
      expect(response.download.status).toBe(DownloadStatus.Idle);
      expect(controller.getDownload(path).url).toBe('https://example.com/recreated.zip');
   });

   it.each(PATH_ACTIONS)('rejects %s for a path with no stored download', async (action) => {
      mockDownloadPlugin();

      await expect(invokeAction(action, '/tmp/missing.zip')).rejects.toThrow('Not Found: /tmp/missing.zip');
   });

   it.each(ACTION_STATUS_CASES)('keeps mocked action responses aligned with action tables for %s from %s', async (action, status) => {
      const path = `/tmp/${action}-${status}.zip`;

      const controller = mockDownloadPlugin({
         downloads: [
            createMockDownloadState(status, { path }),
         ],
      });

      const expectedStatus = getExpectedStatus(action);

      const isAllowed = (allowedActions[status] as readonly DownloadAction[]).includes(action);

      const expectedResultStatus = isAllowed ? expectedStatus : status;

      const expectedFlag = expectedResultStatus === expectedStatus;

      const download = await get(path);

      expect(hasAction(download, action)).toBe(isAllowed);

      const response = await invokeAction(action, path);

      expect(response.expectedStatus).toBe(expectedStatus);
      expect(response.isExpectedStatus).toBe(expectedFlag);
      expect(response.download.status).toBe(expectedResultStatus);
      expect(response.download.path).toBe(path);
      const isRemoved = isAllowed && action === DownloadAction.Cancel;

      expect(controller.listDownloads().map((stored) => { return stored.status; }))
         .toEqual(isRemoved ? [] : [ expectedResultStatus ]);
   });
});
