import { createSlice, type PayloadAction } from "@reduxjs/toolkit";
import type { Calibration, CalShotReport, CalSolved } from "../protocol";

export type SlotState = { status: "empty" } | { status: "taking" } | { status: "done"; report: CalShotReport };

export interface CalibrationState {
  sessionOpen: boolean;
  /** `slots[group][roll]`. */
  slots: SlotState[][];
  solved: CalSolved | null;
  /** `true` once a slot changed after the last solve: per-shot residuals no longer apply. */
  solvedStale: boolean;
  solving: boolean;
  pendingSet: boolean;
  storeId: number;
  calibration: Calibration | null;
  busyMessage: string | null;
}

export const CAL_GROUPS = 14;
export const CAL_ROLLS = 4;

function emptySlots(): SlotState[][] {
  return Array.from({ length: CAL_GROUPS }, () => Array.from({ length: CAL_ROLLS }, (): SlotState => ({ status: "empty" })));
}

const initialState: CalibrationState = {
  sessionOpen: false,
  slots: emptySlots(),
  solved: null,
  solvedStale: false,
  solving: false,
  pendingSet: false,
  storeId: Math.floor(Date.now() / 1000),
  calibration: null,
  busyMessage: null,
};

export const calibrationSlice = createSlice({
  name: "calibration",
  initialState,
  reducers: {
    sessionChanged(state, a: PayloadAction<boolean>) {
      state.sessionOpen = a.payload;
      if (a.payload) {
        state.slots = emptySlots();
        state.solved = null;
        state.pendingSet = false;
      }
    },
    slotTaking(state, a: PayloadAction<{ group: number; roll: number }>) {
      const row = state.slots[a.payload.group];
      if (row) row[a.payload.roll] = { status: "taking" };
    },
    slotDone(state, a: PayloadAction<CalShotReport>) {
      const row = state.slots[a.payload.shot.group];
      if (row) row[a.payload.shot.roll] = { status: "done", report: a.payload };
      state.solvedStale = state.solved !== null;
    },
    slotFailed(state, a: PayloadAction<{ group: number; roll: number }>) {
      const row = state.slots[a.payload.group];
      if (row) row[a.payload.roll] = { status: "empty" };
      state.solvedStale = state.solved !== null;
    },
    slotsReplaced(state, a: PayloadAction<CalShotReport[]>) {
      state.slots = emptySlots();
      for (const r of a.payload) {
        const row = state.slots[r.shot.group];
        if (row && r.shot.roll < CAL_ROLLS) row[r.shot.roll] = { status: "done", report: r };
      }
    },
    solvingChanged(state, a: PayloadAction<boolean>) {
      state.solving = a.payload;
    },
    solvedReceived(state, a: PayloadAction<CalSolved>) {
      state.solved = a.payload;
      state.solvedStale = false;
      state.solving = false;
      state.pendingSet = a.payload.error === null;
    },
    pendingChanged(state, a: PayloadAction<boolean>) {
      state.pendingSet = a.payload;
    },
    storeIdChanged(state, a: PayloadAction<number>) {
      state.storeId = a.payload;
    },
    calibrationReceived(state, a: PayloadAction<Calibration | null>) {
      state.calibration = a.payload;
    },
    busyMessage(state, a: PayloadAction<string | null>) {
      state.busyMessage = a.payload;
    },
  },
});

export const {
  sessionChanged,
  slotTaking,
  slotDone,
  slotFailed,
  slotsReplaced,
  solvingChanged,
  solvedReceived,
  pendingChanged,
  storeIdChanged,
  calibrationReceived,
  busyMessage,
} = calibrationSlice.actions;
