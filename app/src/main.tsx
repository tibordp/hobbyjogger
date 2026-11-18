import React from "react";
import ReactDOM from "react-dom/client";

import App from "@/App";
import { startLifecycle } from "@/lib/lifecycle";
import { startMenu } from "@/lib/menu";
import { loadReference } from "@/lib/reference";
import { startSync } from "@/lib/store";
import "@/index.css";

void startSync();
void loadReference();
startLifecycle();
startMenu();

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
