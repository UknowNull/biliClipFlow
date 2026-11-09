import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import ErrorBoundary from "./components/ErrorBoundary";
import { installGlobalErrorLogging } from "./lib/clientLog";
import { installPointerShim } from "./lib/pointerShim";
import "./index.css";

installPointerShim();
installGlobalErrorLogging();

document.documentElement.classList.add("light");

ReactDOM.createRoot(document.getElementById("root")).render(
  <React.StrictMode>
    <ErrorBoundary>
      <App />
    </ErrorBoundary>
  </React.StrictMode>,
);
