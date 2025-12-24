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
    * @param key The key of the download item to listen for
    * @param listener The callback function to invoke when the download changes
    * @returns A promise with a function to remove this specific listener
    */
   public async addListener(key: string, listener: (download: DownloadWithAnyStatus) => void): Promise<() => void> {
      await this._ensureGlobalListeners();

      if (!this._listeners.has(key)) {
         this._listeners.set(key, new Set());
      }

      const listenersForKey = this._listeners.get(key);

      if (listenersForKey) {
         listenersForKey.add(listener);
      }

      // Return a function to remove this specific listener
      return () => {
         const listeners = this._listeners.get(key);

         if (listeners) {
            listeners.delete(listener);

            // If no more listeners for this key, remove the key from the map.
            if (listeners.size === 0) {
               this._listeners.delete(key);
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
            this._notifyListeners(event.key, event);
         });
      } else {
         this._eventUnlistenFn = await listen<DownloadState<DownloadStatus>>('tauri-plugin-download:changed', (event) => {
            this._notifyListeners(event.payload.key, event.payload);
         });
      }
   }

   private _notifyListeners(key: string, event: DownloadState<DownloadStatus>): void {
      const listeners = this._listeners.get(key);

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
      return DownloadEventManager.shared.addListener(this.key, listener);
   },

   async create(url: string, path: string) {
      return sendAction(DownloadAction.Create, { key: this.key, url, path });
   },

   async start() {
      return sendAction(DownloadAction.Start, { key: this.key });
   },

   async resume() {
      return sendAction(DownloadAction.Resume, { key: this.key });
   },

   async cancel() {
      return sendAction(DownloadAction.Cancel, { key: this.key });
   },

   async pause() {
      return sendAction(DownloadAction.Pause, { key: this.key });
   },
} satisfies AllDownloadActions & ThisType<DownloadState<DownloadStatus>>;

/**
 * Attaches a {@link Download} object with the allowed actions for the given state
 *
 * @param state The de-serialized download state from the plugin
 */
export function attachDownload<S extends DownloadStatus>(state: DownloadState<S>): Download<S> {
   const download = {
      key: state.key,
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
