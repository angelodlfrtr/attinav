import { createSlice, type PayloadAction } from "@reduxjs/toolkit";

export type Page = "dashboard" | "shots" | "stream" | "calibration" | "config" | "firmware" | "console" | "simulator";

export interface Toast {
  id: number;
  level: "info" | "error" | "success";
  text: string;
}

export interface UiState {
  page: Page;
  toasts: Toast[];
}

const initialState: UiState = {
  page: "dashboard",
  toasts: [],
};

let nextToast = 1;

export const uiSlice = createSlice({
  name: "ui",
  initialState,
  reducers: {
    navigate(state, a: PayloadAction<Page>) {
      state.page = a.payload;
    },
    toast(state, a: PayloadAction<{ level: Toast["level"]; text: string }>) {
      state.toasts.push({ id: nextToast, ...a.payload });
      nextToast += 1;
      if (state.toasts.length > 5) state.toasts.shift();
    },
    dismissToast(state, a: PayloadAction<number>) {
      state.toasts = state.toasts.filter((t) => t.id !== a.payload);
    },
  },
});

export const { navigate, toast, dismissToast } = uiSlice.actions;
