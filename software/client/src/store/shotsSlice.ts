import { createSlice, type PayloadAction } from "@reduxjs/toolkit";
import type { ShotRaw, ShotResult } from "../protocol";

export interface ShotRecord {
  result: ShotResult;
  raw: ShotRaw[];
  receivedAt: number;
  note: string;
}

export interface ShotsState {
  history: ShotRecord[];
  inProgress: boolean;
  startedAt: number | null;
  expectedMs: number;
  selected: number | null;
  wantRaw: boolean;
  noQuiet: boolean;
  magSamples: number;
  cycleCount: number;
}

const initialState: ShotsState = {
  history: [],
  inProgress: false,
  startedAt: null,
  expectedMs: 0,
  selected: null,
  wantRaw: true,
  noQuiet: false,
  magSamples: 0,
  cycleCount: 0,
};

const MAX_HISTORY = 500;

export const shotsSlice = createSlice({
  name: "shots",
  initialState,
  reducers: {
    shotStarted(state, a: PayloadAction<{ expectedMs: number }>) {
      state.inProgress = true;
      state.startedAt = Date.now();
      state.expectedMs = a.payload.expectedMs;
    },
    shotFinished(state) {
      state.inProgress = false;
      state.startedAt = null;
    },
    shotRecorded(state, a: PayloadAction<{ result: ShotResult; raw: ShotRaw[] }>) {
      state.history.unshift({ ...a.payload, receivedAt: Date.now(), note: "" });
      if (state.history.length > MAX_HISTORY) state.history.length = MAX_HISTORY;
      state.selected = a.payload.result.shot_id;
      state.inProgress = false;
      state.startedAt = null;
    },
    shotSelected(state, a: PayloadAction<number | null>) {
      state.selected = a.payload;
    },
    shotNoted(state, a: PayloadAction<{ shotId: number; note: string }>) {
      const r = state.history.find((h) => h.result.shot_id === a.payload.shotId);
      if (r) r.note = a.payload.note;
    },
    historyCleared(state) {
      state.history = [];
      state.selected = null;
    },
    shotOptions(state, a: PayloadAction<Partial<Pick<ShotsState, "wantRaw" | "noQuiet" | "magSamples" | "cycleCount">>>) {
      Object.assign(state, a.payload);
    },
  },
});

export const { shotStarted, shotFinished, shotRecorded, shotSelected, shotNoted, historyCleared, shotOptions } = shotsSlice.actions;
