import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import App from "./App";
import { MenuBarPanel } from "./components/MenuBarPanel";
import "./styles.css";

const inPanel = new URLSearchParams(window.location.search).has("panel");

createRoot(document.getElementById("root")!).render(
  <StrictMode>{inPanel ? <MenuBarPanel /> : <App />}</StrictMode>,
);
