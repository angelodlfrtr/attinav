import { createSlice, type PayloadAction } from "@reduxjs/toolkit";
import { DEFAULT_CONFIG, type CalInfo, type DeviceConfig, type DeviceEvent, type Info } from "../protocol";

export interface DeviceState {
  info: Info | null;
  infoAt: number | null;
  config: DeviceConfig;
  configFromDevice: boolean;
  calInfo: CalInfo | null;
  lastEvent: DeviceEvent | null;
}

const initialState: DeviceState = {
  info: null,
  infoAt: null,
  config: DEFAULT_CONFIG,
  configFromDevice: false,
  calInfo: null,
  lastEvent: null,
};

export const deviceSlice = createSlice({
  name: "device",
  initialState,
  reducers: {
    infoReceived(state, a: PayloadAction<Info>) {
      state.info = a.payload;
      state.infoAt = Date.now();
    },
    configReceived(state, a: PayloadAction<DeviceConfig>) {
      state.config = a.payload;
      state.configFromDevice = true;
    },
    calInfoReceived(state, a: PayloadAction<CalInfo>) {
      state.calInfo = a.payload;
    },
    eventReceived(state, a: PayloadAction<DeviceEvent>) {
      state.lastEvent = a.payload;
    },
    deviceForgotten(state) {
      state.info = null;
      state.infoAt = null;
      state.configFromDevice = false;
      state.calInfo = null;
    },
  },
});

export const { infoReceived, configReceived, calInfoReceived, eventReceived, deviceForgotten } = deviceSlice.actions;
