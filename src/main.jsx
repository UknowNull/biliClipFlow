import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { installPointerShim } from "./lib/pointerShim";
import "./index.css";

installPointerShim();

document.documentElement.classList.add("light");

ReactDOM.createRoot(document.getElementById("root")).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
