import { createSlice, type PayloadAction } from "@reduxjs/toolkit";
import type { DeviceKind } from "../protocol";

export interface ConsoleEntry {
  id: number;
  at: number;
  direction: "rx" | "tx" | "sys";
  seq: number;
  kind: string;
  summary: string;
  json: string;
}

export interface ConsoleState {
  entries: ConsoleEntry[];
  paused: boolean;
  hideSamples: boolean;
  filter: string;
  /** Per-kind counters since connect. */
  counts: Partial<Record<DeviceKind | "dropped", number>>;
}

const initialState: ConsoleState = {
  entries: [],
  paused: false,
  hideSamples: true,
  filter: "",
  counts: {},
};

const MAX_ENTRIES = 2000;
let nextId = 1;

export const consoleSlice = createSlice({
  name: "console",
  initialState,
  reducers: {
    logged(state, a: PayloadAction<Omit<ConsoleEntry, "id" | "at">>) {
      if (state.paused) return;
      state.entries.push({ ...a.payload, id: nextId, at: Date.now() });
      nextId += 1;
      if (state.entries.length > MAX_ENTRIES) state.entries.splice(0, state.entries.length - MAX_ENTRIES);
    },
    counted(state, a: PayloadAction<DeviceKind>) {
      state.counts[a.payload] = (state.counts[a.payload] ?? 0) + 1;
    },
    cleared(state) {
      state.entries = [];
      state.counts = {};
    },
    paused(state, a: PayloadAction<boolean>) {
      state.paused = a.payload;
    },
    hideSamples(state, a: PayloadAction<boolean>) {
      state.hideSamples = a.payload;
    },
    filterChanged(state, a: PayloadAction<string>) {
      state.filter = a.payload;
    },
  },
});

export const consoleActions = consoleSlice.actions;
