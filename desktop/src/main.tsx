import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { DesktopRuntime } from "./DesktopRuntime";
import "./styles.css";

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <DesktopRuntime><App /></DesktopRuntime>
  </React.StrictMode>,
);
