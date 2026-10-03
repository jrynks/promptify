const window = workspace.activeWindow;
const payload = window
    ? { id: String(window.internalId), pid: window.pid || 0, class: String(window.resourceClass || ""), caption: String(window.caption || "") }
    : { id: "", pid: 0, class: "", caption: "" };
callDBus("%SERVICE%", "/dev/promptify/Focus", "dev.promptify.Focus", "Snapshot", "%REQUEST%", JSON.stringify(payload));
