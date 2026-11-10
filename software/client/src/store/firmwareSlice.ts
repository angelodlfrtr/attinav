import { createSlice, type PayloadAction } from "@reduxjs/toolkit";
import type { FirmwareFile, FwProgress } from "../api";

export interface FirmwareState {
  path: string | null;
  file: FirmwareFile | null;
  version: [number, number, number];
  progress: FwProgress | null;
  running: boolean;
  log: string[];
}

const initialState: FirmwareState = {
  path: null,
  file: null,
  version: [0, 1, 1],
  progress: null,
  running: false,
  log: [],
};

export const firmwareSlice = createSlice({
  name: "firmware",
  initialState,
  reducers: {
    fileChosen(state, a: PayloadAction<{ path: string; file: FirmwareFile }>) {
      state.path = a.payload.path;
      state.file = a.payload.file;
      state.progress = null;
    },
    versionChanged(state, a: PayloadAction<[number, number, number]>) {
      state.version = a.payload;
    },
    updateStarted(state) {
      state.running = true;
      state.progress = null;
      state.log = [];
    },
    progressReceived(state, a: PayloadAction<FwProgress>) {
      state.progress = a.payload;
      if (a.payload.stage !== "transfer") {
        state.log.push(`${new Date().toLocaleTimeString()}  ${a.payload.stage}${a.payload.message ? `: ${a.payload.message}` : ""}`);
      }
      if (a.payload.stage === "done" || a.payload.stage === "failed") state.running = false;
    },
    updateFinished(state, a: PayloadAction<string | null>) {
      state.running = false;
      if (a.payload) state.log.push(`${new Date().toLocaleTimeString()}  ${a.payload}`);
    },
  },
});

export const { fileChosen, versionChanged, updateStarted, progressReceived, updateFinished } = firmwareSlice.actions;
