<template>
   <div class="download-item">
      <div class="item-header">
         <h3 class="item-name">{{ currentDownload?.path?.split('/').pop() }}</h3>
         <div class="item-actions" v-if="showActions">
            <button class="btn create-btn" type="button" @click="doCreate" v-if="canCreate">Create</button>
            <button class="btn start-btn" type="button" @click="doAction(DownloadAction.Start)" v-if="canStart">Start</button>
            <button class="btn cancel-btn" type="button" @click="doAction(DownloadAction.Cancel)" v-if="canCancel">Cancel</button>
            <button class="btn pause-btn" type="button" @click="doAction(DownloadAction.Pause)" v-if="canPause">Pause</button>
            <button class="btn resume-btn" type="button" @click="doAction(DownloadAction.Resume)" v-if="canResume">Resume</button>
         </div>
      </div>
      <p class="item-path">{{ currentDownload?.path }}</p>
      <div class="progress-bar">
         <div class="progress" :style="{ width: currentDownload.progress + '%' }" />
      </div>
      <div class="item-info">
         <p class="state-text">State: {{ currentDownload.status }}</p>
         <p class="byte-text">{{ byteCount }}</p>
         <p class="metered-text" v-if="currentDownload.status !== DownloadStatus.Pending">
            Metered: {{ currentDownload.options.allowMetered ? 'allowed' : 'blocked' }}
         </p>
         <p class="progress-text">{{ Math.round(currentDownload.progress) }}%</p>
      </div>
   </div>
</template>

<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from 'vue';
import {
   hasAction,
   hasAnyAction,
   DownloadAction,
   DownloadStatus,
   type DownloadWithAnyStatus,
   type Download,
   type UnexpectedStatusesForAction,
   type DownloadActionResponse,
} from 'tauri-plugin-download';
import { UnlistenFn } from '@tauri-apps/api/event';

const props = defineProps<{ download: DownloadWithAnyStatus, url?: string, allowMetered?: boolean }>(),
      currentDownload = ref<DownloadWithAnyStatus>(props.download),
      showActions = computed(() => { return hasAnyAction(currentDownload.value); }),
      canCreate = computed(() => { return hasAction(currentDownload.value, DownloadAction.Create); }),
      canStart = computed(() => { return hasAction(currentDownload.value, DownloadAction.Start); }),
      canCancel = computed(() => { return hasAction(currentDownload.value, DownloadAction.Cancel); }),
      canPause = computed(() => { return hasAction(currentDownload.value, DownloadAction.Pause); }),
      canResume = computed(() => { return hasAction(currentDownload.value, DownloadAction.Resume); });

const BYTE_UNITS = [ 'B', 'KB', 'MB', 'GB', 'TB' ];

function formatBytes(bytes: number): string {
   let value = bytes,
       unitIndex = 0;

   while (value >= 1024 && unitIndex < BYTE_UNITS.length - 1) {
      value = value / 1024;
      unitIndex = unitIndex + 1;
   }

   return `${value.toFixed(unitIndex === 0 ? 0 : 1)} ${BYTE_UNITS[unitIndex]}`;
}

// A server that omits the content length leaves totalBytes null, so the total
// is not always known even while bytes are arriving. A strict null check is safe:
// attachDownload() normalizes an absent key to null before it reaches here.
const byteCount = computed<string>(() => {
   const { receivedBytes, totalBytes } = currentDownload.value,
         total = totalBytes === null ? 'unknown' : formatBytes(totalBytes);

   return `${formatBytes(receivedBytes)} / ${total}`;
});


let unlisten: UnlistenFn | undefined;

onMounted(listenToEvents);
onUnmounted(() => { return unlisten?.(); });

async function listenToEvents(): Promise<void> {
   if (unlisten || !hasAction(currentDownload.value, DownloadAction.Listen)) {
      return;
   }
   unlisten = await currentDownload.value.listen((updated) => {
      currentDownload.value = updated;
   });
}

function onError(error: unknown): void {
   console.error(error);
}

type StatusHandlers<A extends DownloadAction> = Partial<{
   [S in UnexpectedStatusesForAction<A>]: (actualState: Download<S>) => void;
}>;

type ActionHandlers = Partial<{
   [K in DownloadAction]: StatusHandlers<K>;
}>;

const unexpectedStatusHandlers: ActionHandlers = {
   [DownloadAction.Start]: {
      [DownloadStatus.Canceled]: () => {
         // Tried to start the download but it was canceled instead
      },
   },
   [DownloadAction.Resume]: {
      [DownloadStatus.Canceled]: () => {
         // Tried to start the download but it was canceled instead
      },
   },

   [DownloadAction.Cancel]: {
      [DownloadStatus.Completed]: (): void => {
         // You'll probably want to delete the file since the user wanted to cancel
         // the download but wasn't able to before it completed
      },
      [DownloadStatus.InProgress]: (): void => {
         // There was a problem canceling the download
      },
   },

   [DownloadAction.Pause]: {
      [DownloadStatus.InProgress]: (): void => {
         // There was a problem pausing the download
      },
      [DownloadStatus.Completed]: (): void => {
         // The user tried to pause a completed download. This probably doesn't matter as
         // much as the other cases
      },
   },
};


function handleUnexpectedStatus(action: DownloadAction, result: DownloadActionResponse<DownloadAction>): void {
   const handlers = action in unexpectedStatusHandlers ? unexpectedStatusHandlers[action] : undefined;

   if (!handlers) {
      return;
   }

   const handler = (handlers as Partial<Record<DownloadStatus, (download: DownloadWithAnyStatus) => void>>)[result.download.status];

   if (handler) {
      handler(result.download);
   }
}

async function doCreate(): Promise<void> {
   if (!hasAction(currentDownload.value, DownloadAction.Create) || !props.url) {
      return;
   }

   try {
      const result = await currentDownload.value.create(props.url, {
         allowMetered: props.allowMetered ?? true,
      });

      currentDownload.value = result.download;

      if (!result.isExpectedStatus) {
         handleUnexpectedStatus(DownloadAction.Create, result);
      }
   } catch(error) {
      onError(error);
   }
}

type NoArgAction = Exclude<DownloadAction, DownloadAction.Listen | DownloadAction.Create>;
async function doAction<A extends NoArgAction>(action: A): Promise<void> {
   if (!hasAction(currentDownload.value, action)) {
      return;
   }

   try {
      const result = await currentDownload.value[action]();

      currentDownload.value = result.download;

      if (!result.isExpectedStatus) {
         handleUnexpectedStatus(action, result);
      }
   } catch(error) {
      onError(error);
   }
}
</script>

<style scoped>
  .download-item {
    border: 1px solid #ddd;
    border-radius: 8px;
    padding: 15px;
    margin-bottom: 20px;
    box-shadow: 0 4px 6px rgba(0, 0, 0, 0.1);
  }

  .item-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-bottom: 10px;
  }

  .item-name {
    font-size: 18px;
    font-weight: bold;
    margin: 0;
  }

  .item-actions button {
    margin-left: 5px;
  }

  .btn {
    padding: 6px;
    margin: 10px 5px;
    border: none;
    background-color: #007bff;
  }

  .item-info {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin: 0;
  }

  .progress-text {
    font-size: 14px;
    color: #555;
    margin: 0;
  }

  .byte-text {
    font-size: 14px;
    color: #555;
    margin: 0;
    white-space: nowrap;
  }

  .progress-bar {
    background: #f0f0f0;
    border-radius: 4px;
    height: 10px;
    width: 100%;
    overflow: hidden;
    margin-bottom: 0;
  }

  .progress {
    background: #007bff;
    height: 100%;
    transition: width 0.3s;
  }

  .state-text {
    font-size: 14px;
    color: #555;
  }

  .item-path {
    font-size: 12px;
    color: #888;
    margin: 0 0 10px 0;
    text-align: left;
    word-break: break-all;
  }
  </style>
