import React from "react";
import ReactDOM from "react-dom/client";
import { Provider } from "react-redux";
import "./index.css";
import { store } from "./store";
import { startListeners } from "./listeners";
import { App } from "./components/App";

const root = document.getElementById("root");
if (!root) throw new Error("missing #root");

void startListeners(store);

ReactDOM.createRoot(root).render(
  <React.StrictMode>
    <Provider store={store}>
      <App />
    </Provider>
  </React.StrictMode>,
);
