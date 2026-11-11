import React from "react";
import ReactDOM from "react-dom/client";
import "./styles/tokens.css";
import "./styles/base.css";
import App from "./App";
import "./styles/shell.css";
import "./styles/dashboard.css";
import "./styles/settings.css";
import { initializeTheme } from "./theme";

void initializeTheme();

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
