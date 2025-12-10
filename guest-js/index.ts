import { invoke, addPluginListener } from '@tauri-apps/api/core';
import { listen, UnlistenFn } from '@tauri-apps/api/event';

/**
* Represents the state of a download operation.
* Enum values are camel-cased to match the Rust and mobile plugin implementations.
*/
export enum DownloadState {
  UNKNOWN = 'unknown',
  CREATED = 'created',
  IN_PROGRESS = 'inProgress',
  PAUSED = 'paused',
  CANCELLED = 'cancelled',
  COMPLETED = 'completed'
}

/**
 * Represents a download item.
 */
export interface DownloadItem {
  key: string;
  url: string;
  path: string;
  progress: number;
  state: DownloadState;
}

/**
 * Manages subscriptions to download events from Rust and
 * mobile plugins (iOS/Android), and dispatching these events
 * to registered listeners.
 */
export class DownloadEventManager {
   public static shared: DownloadEventManager = new DownloadEventManager();
   private _listeners: Map<string, Set<(download: Download) => void>> = new Map();
   private _eventUnlistenFn: UnlistenFn | null = null;
   private _pluginListener: { unregister: () => void } | null = null;

   private constructor() { }

   /**
    * Adds a listener for download events.
    * @param key - The key of the download item to listen for.
    * @param listener - The callback function to invoke when the download changes.
    * @returns A promise with a function to remove this specific listener.
    */
   public async addListener(key: string, listener: (download: Download) => void): Promise<() => void> {
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

      // Check if the plugin is running in a native environment (iOS)
      // or is the shared Rust implementation (desktop/Android).
      const isNative = await invoke<boolean>('plugin:download|is_native');

      if (isNative) {
         this._pluginListener = await addPluginListener('download', 'changed', (event: DownloadItem) => {
            this._notifyListeners(event.key, event);
         });
      } else {
         this._eventUnlistenFn = await listen<DownloadItem>('tauri-plugin-download:changed', (event) => {
            this._notifyListeners(event.payload.key, event.payload);
         });
      }
   }

   private _notifyListeners(key: string, event: DownloadItem): void {
      const listeners = this._listeners.get(key);

      if (listeners) {
         // eslint-disable-next-line @typescript-eslint/no-use-before-define
         listeners.forEach((listener) => { return listener(createDownload(event)); });
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

/**
 * Base class for all download states.
 * Contains common properties and the listen method.
 */
abstract class DownloadBase implements DownloadItem {
   public readonly key: string;
   public readonly url: string;
   public readonly path: string;
   public readonly progress: number;
   public abstract readonly state: DownloadState;

   protected constructor(item: DownloadItem) {
      this.key = item.key;
      this.url = item.url;
      this.path = item.path;
      this.progress = item.progress;
   }

   /**
   * Listen for changes to the download.
   * To avoid memory leaks, the `unlisten` function returned by the promise
   * should be called when no longer required.
   * @param onChanged - Callback function invoked when the download has changed.
   * @returns A promise with a function to remove the download listener.
   *
   * @example
   * ```ts
   * const unlisten = await download.listen((updatedDownload) => {
   *   console.log('Download:', updatedDownload);
   *   if (updatedDownload.state === DownloadState.PAUSED) {
   *     updatedDownload.resume(); // TypeScript knows this is valid
   *   }
   * });
   *
   * // To stop listening
   * unlisten();
   * ```
   */
   public async listen(listener: (download: Download) => void): Promise<UnlistenFn> {
      return DownloadEventManager.shared.addListener(this.key, listener);
   }
}

/**
 * A download that has been cancelled.
 * Terminal state - no further actions available.
 */
export class CancelledDownload extends DownloadBase {
   public readonly state = DownloadState.CANCELLED;

   public constructor(item: DownloadItem) {
      super(item);
   }
}

/**
 * A download that has completed successfully.
 * Terminal state - no further actions available.
 */
export class CompletedDownload extends DownloadBase {
   public readonly state = DownloadState.COMPLETED;

   public constructor(item: DownloadItem) {
      super(item);
   }
}

/**
 * A download in an unknown state.
 * This may occur if the plugin returns an unrecognized state.
 */
export class UnknownDownload extends DownloadBase {
   public readonly state = DownloadState.UNKNOWN;

   public constructor(item: DownloadItem) {
      super(item);
   }
}

/**
 * A download that has been paused.
 * Can be resumed or cancelled.
 */
export class PausedDownload extends DownloadBase {
   public readonly state = DownloadState.PAUSED;

   public constructor(item: DownloadItem) {
      super(item);
   }

   /**
   * Resumes the download.
   * @returns A promise with the updated download.
   */
   // eslint-disable-next-line @typescript-eslint/no-use-before-define
   public async resume(): Promise<ActiveDownload> {
      // eslint-disable-next-line @typescript-eslint/no-use-before-define
      return new ActiveDownload(await invoke('plugin:download|resume', { key: this.key }));
   }

   /**
   * Cancels the download.
   * @returns A promise with the updated download.
   */
   public async cancel(): Promise<CancelledDownload> {
      return new CancelledDownload(await invoke('plugin:download|cancel', { key: this.key }));
   }
}

/**
 * A download that is currently in progress.
 * Can be paused or cancelled.
 */
export class ActiveDownload extends DownloadBase {
   public readonly state = DownloadState.IN_PROGRESS;

   public constructor(item: DownloadItem) {
      super(item);
   }

   /**
   * Pauses the download.
   * @returns A promise with the updated download.
   */
   public async pause(): Promise<PausedDownload> {
      return new PausedDownload(await invoke('plugin:download|pause', { key: this.key }));
   }

   /**
   * Cancels the download.
   * @returns A promise with the updated download.
   */
   public async cancel(): Promise<CancelledDownload> {
      return new CancelledDownload(await invoke('plugin:download|cancel', { key: this.key }));
   }
}

/**
 * A download that has been created but not yet started.
 * Can be started or cancelled.
 */
export class CreatedDownload extends DownloadBase {
   public readonly state = DownloadState.CREATED;

   public constructor(item: DownloadItem) {
      super(item);
   }

   /**
   * Starts the download.
   * @returns A promise with the updated download.
   */
   public async start(): Promise<ActiveDownload> {
      return new ActiveDownload(await invoke('plugin:download|start', { key: this.key }));
   }

   /**
   * Cancels the download.
   * @returns A promise with the updated download.
   */
   public async cancel(): Promise<CancelledDownload> {
      return new CancelledDownload(await invoke('plugin:download|cancel', { key: this.key }));
   }
}

/**
 * Union type representing a download in any state.
 * Check the `state` property to narrow to a specific type.
 *
 * @example
 * ```ts
 * if (download.state === DownloadState.CREATED) {
 *   await download.start(); // TypeScript knows start() is available
 * }
 * ```
 */
export type Download =
   | CreatedDownload
   | ActiveDownload
   | PausedDownload
   | CancelledDownload
   | CompletedDownload
   | UnknownDownload;

/**
 * Creates the appropriate Download subclass based on the item's state.
 * @param item - The download item from the plugin.
 * @returns The typed download instance.
 */
function createDownload(item: DownloadItem): Download {
   switch (item.state) {
      case DownloadState.CREATED: {
         return new CreatedDownload(item);
      }
      case DownloadState.IN_PROGRESS: {
         return new ActiveDownload(item);
      }
      case DownloadState.PAUSED: {
         return new PausedDownload(item);
      }
      case DownloadState.CANCELLED: {
         return new CancelledDownload(item);
      }
      case DownloadState.COMPLETED: {
         return new CompletedDownload(item);
      }
      default: {
         return new UnknownDownload(item);
      }
   }
}

/**
 * Creates a download operation.
 *
 * @param key - The key identifier.
 * @param url - The download URL for the resource.
 * @param path - The download path on the filesystem.
 * @returns - The download operation.
 */
export async function create(key: string, url: string, path: string): Promise<CreatedDownload> {
   return new CreatedDownload(await invoke<DownloadItem>('plugin:download|create', { key, url, path }));
}

/**
 * Gets all download operations.
 *
 * @returns - The list of download operations.
 */
export async function list(): Promise<Download[]> {
   return (await invoke<DownloadItem[]>('plugin:download|list'))
      .map((item) => { return createDownload(item); });
}

/**
 * Gets a download operation.
 *
 * @param key - The key identifier.
 * @returns - The download operation.
 */
export async function get(key: string): Promise<Download> {
   return createDownload(await invoke<DownloadItem>('plugin:download|get', { key }));
}
