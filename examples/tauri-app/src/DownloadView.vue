<template>
    <div class="download-item">
        <div class="item-header">
        <h3 class="item-name">{{ download?.key }}</h3>
        <div class="item-actions" v-if="!(isCancelled || isCompleted)">
            <button class="btn start-btn" @click="startDownload" v-if="canStart">Start</button>
            <button class="btn cancel-btn" @click="cancelDownload" v-if="canCancel">Cancel</button>
            <button class="btn pause-btn" @click="pauseDownload" v-if="canPause">Pause</button>
            <button class="btn resume-btn" @click="resumeDownload" v-if="canResume">Resume</button>
        </div>
        </div>
        <div class="progress-bar">
          <div class="progress" :style="{ width: progress + '%' }"></div>
        </div>
        <p class="state-text">State: {{ isCompleted ? "Completed" : download?.state }}</p>
    </div>
</template>
  
<script setup lang="ts">
import { computed, onMounted, onUnmounted, PropType, ref } from 'vue';
import { Download, DownloadState } from 'tauri-plugin-download-api'

const props = defineProps({
   model: {
      type: Object as PropType<Download>,
      required: true,
   },
});

const download = ref<Download>(),
    state = ref<DownloadState>(),
    progress = ref<number>(),
    isCancelled = computed(() => state.value === DownloadState.CANCELLED),
    isCompleted = computed(() => state.value === DownloadState.COMPLETED),
    canStart = computed(() => download.value?.state === DownloadState.CREATED),
    canCancel = computed(() => download.value?.state === DownloadState.CREATED || download.value?.state === DownloadState.IN_PROGRESS || download.value?.state === DownloadState.PAUSED),
    canPause = computed(() => download.value?.state === DownloadState.IN_PROGRESS),
    canResume = computed(() => download.value?.state === DownloadState.PAUSED);

onMounted(async () => {
    download.value = props.model;
    progress.value = props.model.progress;

    props.model.onState = (value: DownloadState) => {
      state.value = value;
    }

    props.model.onProgress = (value: number) => {
      progress.value = value;
    };
})

onUnmounted(async () => props.model.detach())

async function startDownload() {
    download.value = await props.model.start();
}

async function cancelDownload() {
    download.value = await props.model.cancel();
}

async function pauseDownload() {
    download.value = await props.model.pause();
}

async function resumeDownload() {
    download.value = await props.model.resume();
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
  
  .progress-bar {
    background: #f0f0f0;
    border-radius: 4px;
    height: 10px;
    width: 100%;
    overflow: hidden;
    margin-bottom: 5px;
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
  </style>