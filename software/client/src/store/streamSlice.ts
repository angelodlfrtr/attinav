import { createSlice, type PayloadAction } from "@reduxjs/toolkit";
import { STREAM, type Temps } from "../protocol";

export interface StreamState {
  running: boolean;
  rateHz: number;
  effectiveRateHz: number | null;
  sensors: number;
  total: number;
  measuredRateHz: number;
  temps: Temps | null;
  windowSeconds: number;
}

const initialState: StreamState = {
  running: false,
  rateHz: 20,
  effectiveRateHz: null,
  sensors: STREAM.MAG1 | STREAM.MAG2 | STREAM.MAG3 | STREAM.ACC1 | STREAM.ACC2 | STREAM.TEMPS,
  total: 0,
  measuredRateHz: 0,
  temps: null,
  windowSeconds: 10,
};

export const streamSlice = createSlice({
  name: "stream",
  initialState,
  reducers: {
    streamStarted(state, a: PayloadAction<{ effectiveRateHz: number }>) {
      state.running = true;
      state.effectiveRateHz = a.payload.effectiveRateHz;
    },
    streamStopped(state) {
      state.running = false;
      state.effectiveRateHz = null;
    },
    streamOptions(state, a: PayloadAction<Partial<Pick<StreamState, "rateHz" | "sensors" | "windowSeconds">>>) {
      Object.assign(state, a.payload);
    },
    streamCounters(state, a: PayloadAction<{ total: number; measuredRateHz: number }>) {
      state.total = a.payload.total;
      state.measuredRateHz = a.payload.measuredRateHz;
    },
    tempsReceived(state, a: PayloadAction<Temps>) {
      state.temps = a.payload;
    },
  },
});

export const { streamStarted, streamStopped, streamOptions, streamCounters, tempsReceived } = streamSlice.actions;
