import React from "react";
import ReactDOM from "react-dom/client";

import App from "@/App";
import { startSync } from "@/lib/store";
import "@/index.css";

void startSync();

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
