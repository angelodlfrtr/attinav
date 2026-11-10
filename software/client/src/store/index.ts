import { configureStore } from "@reduxjs/toolkit";
import { useDispatch, useSelector, type TypedUseSelectorHook } from "react-redux";
import { linkSlice } from "./linkSlice";
import { deviceSlice } from "./deviceSlice";
import { shotsSlice } from "./shotsSlice";
import { streamSlice } from "./streamSlice";
import { calibrationSlice } from "./calibrationSlice";
import { firmwareSlice } from "./firmwareSlice";
import { consoleSlice } from "./consoleSlice";
import { uiSlice } from "./uiSlice";

export const store = configureStore({
  reducer: {
    link: linkSlice.reducer,
    device: deviceSlice.reducer,
    shots: shotsSlice.reducer,
    stream: streamSlice.reducer,
    calibration: calibrationSlice.reducer,
    firmware: firmwareSlice.reducer,
    console: consoleSlice.reducer,
    ui: uiSlice.reducer,
  },
});

export type RootState = ReturnType<typeof store.getState>;
export type AppDispatch = typeof store.dispatch;
export const useAppDispatch: () => AppDispatch = useDispatch;
export const useAppSelector: TypedUseSelectorHook<RootState> = useSelector;
