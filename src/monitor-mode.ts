const BODY_CLASS = "monitor-mode";
const DRAWER_ID  = "output-drawer";

let _active        = false;
let _drawerWasOpen = false;

export function isMonitorModeActive(): boolean {
  return _active;
}

export function enterMonitorMode(): void {
  if (_active) return;
  const drawer = document.getElementById(DRAWER_ID);
  _drawerWasOpen = !!drawer && !drawer.classList.contains("hidden");
  drawer?.classList.add("hidden");
  document.body.classList.add(BODY_CLASS);
  _active = true;
}

export function exitMonitorMode(): void {
  if (!_active) return;
  document.body.classList.remove(BODY_CLASS);
  document.getElementById(DRAWER_ID)?.classList.toggle("hidden", !_drawerWasOpen);
  _active = false;
}
