import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { addPluginListener, invoke } from '@tauri-apps/api/core';
import {
   AllDownloadActions, allowedActions, Download, DownloadAction, DownloadActionResponse, DownloadState,
   DownloadStatus, DownloadWithAnyStatus,
} from './types';

/**
 * Manages subscriptions to download events from Rust and mobile plugins (iOS/Android),
 * and dispatching these events to registered listeners.
 */
class DownloadEventManager {
   public static shared: DownloadEventManager = new DownloadEventManager();
   private _listeners: Map<string, Set<(download: DownloadWithAnyStatus) => void>> = new Map();
   private _eventUnlistenFn: UnlistenFn | null = null;
   private _pluginListener: { unregister: () => void } | null = null;

   private constructor() { }

   /**
    * Adds a listener for download events
    *
    * @param path The path of the download item to listen for
    * @param listener The callback function to invoke when the download changes
    * @returns A promise with a function to remove this specific listener
    */
   public async addListener(path: string, listener: (download: DownloadWithAnyStatus) => void): Promise<() => void> {
      await this._ensureGlobalListeners();

      if (!this._listeners.has(path)) {
         this._listeners.set(path, new Set());
      }

      const listenersForKey = this._listeners.get(path);

      if (listenersForKey) {
         listenersForKey.add(listener);
      }

      // Return a function to remove this specific listener
      return () => {
         const listeners = this._listeners.get(path);

         if (listeners) {
            listeners.delete(listener);

            // If no more listeners for this path, remove the path from the map.
            if (listeners.size === 0) {
               this._listeners.delete(path);
            }
         }

         this._cleanupGlobalListeners();
      };
   }

   private async _ensureGlobalListeners(): Promise<void> {
      if (this._eventUnlistenFn || this._pluginListener) {
         return;
      }

      // Check if the plugin is running in a native environment (iOS) or is the shared
      // Rust implementation (desktop/Android).
      const isNative = await invoke<boolean>('plugin:download|is_native');

      if (isNative) {
         this._pluginListener = await addPluginListener('download', 'changed', (event: DownloadState<DownloadStatus>) => {
            this._notifyListeners(event.path, event);
         });
      } else {
         this._eventUnlistenFn = await listen<DownloadState<DownloadStatus>>('tauri-plugin-download:changed', (event) => {
            this._notifyListeners(event.payload.path, event.payload);
         });
      }
   }

   private _notifyListeners(path: string, event: DownloadState<DownloadStatus>): void {
      const listeners = this._listeners.get(path);

      if (listeners) {
         // eslint-disable-next-line @typescript-eslint/no-use-before-define
         listeners.forEach((listener) => { return listener(attachDownload(event)); });
      }
   }

   private _cleanupGlobalListeners(): void {
      if (this._listeners.size > 0) {
         return;
      }

      if (this._eventUnlistenFn) {
         this._eventUnlistenFn();
         this._eventUnlistenFn = null;
      }

      if (this._pluginListener) {
         this._pluginListener.unregister();
         this._pluginListener = null;
      }
   }
}

async function sendAction<A extends DownloadAction>(action: A, args: Record<string, unknown>): Promise<DownloadActionResponse<A>> {
   const response = await invoke<DownloadActionResponse<A>>('plugin:download|' + action, args);

   response.download = attachDownload(response.download);

   return response;
}

const actions = {
   listen(listener: (download: DownloadWithAnyStatus) => void): Promise<UnlistenFn> {
      return DownloadEventManager.shared.addListener(this.path, listener);
   },

   async create(url: string) {
      return sendAction(DownloadAction.Create, { path: this.path, url });
   },

   async start() {
      return sendAction(DownloadAction.Start, { path: this.path });
   },

   async resume() {
      return sendAction(DownloadAction.Resume, { path: this.path });
   },

   async pause() {
      return sendAction(DownloadAction.Pause, { path: this.path });
   },

   async cancel() {
      return sendAction(DownloadAction.Cancel, { path: this.path });
   },
} satisfies AllDownloadActions & ThisType<DownloadState<DownloadStatus>>;

/**
 * Attaches a {@link Download} object with the allowed actions for the given state
 *
 * @param state The de-serialized download state from the plugin
 */
export function attachDownload<S extends DownloadStatus>(state: DownloadState<S>): Download<S> {
   const download = {
      url: state.url,
      path: state.path,
      progress: state.progress,
      status: state.status,
   } satisfies DownloadState<S>;

   const actionsForDownload = allowedActions[state.status];

   for (const actionName of actionsForDownload) {
      Object.defineProperty(download, actionName, {
         value: actions[actionName],
      });
   }

   return download as Download<S>;
}
