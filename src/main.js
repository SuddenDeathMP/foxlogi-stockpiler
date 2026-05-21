// Thin frontend: an API-key field, a file picker, and an activity log.
// All real work (watch -> parse -> extract -> POST) happens in Rust.

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const els = {};
let saveTimer = null;

function fileName(path) {
  return path.split(/[\\/]/).pop() || path;
}

function setKeyStatus(text, kind = "") {
  els.keyStatus.textContent = text;
  els.keyStatus.className = `status ${kind}`;
  if (text) {
    setTimeout(() => {
      if (els.keyStatus.textContent === text) els.keyStatus.textContent = "";
    }, 2500);
  }
}

function renderFiles(files) {
  els.fileList.innerHTML = "";
  if (!files.length) {
    const li = document.createElement("li");
    li.className = "empty";
    li.textContent = "No files watched yet. Click “Add files…”.";
    els.fileList.appendChild(li);
    return;
  }

  for (const path of files) {
    const li = document.createElement("li");
    li.className = "file";

    const info = document.createElement("div");
    info.className = "file-info";
    const name = document.createElement("span");
    name.className = "file-name";
    name.textContent = fileName(path);
    const full = document.createElement("span");
    full.className = "file-path";
    full.textContent = path;
    full.title = path;
    info.append(name, full);

    const actions = document.createElement("div");
    actions.className = "file-actions";

    const syncBtn = document.createElement("button");
    syncBtn.type = "button";
    syncBtn.className = "ghost";
    syncBtn.textContent = "Sync now";
    syncBtn.addEventListener("click", async () => {
      syncBtn.disabled = true;
      try {
        const count = await invoke("sync_now", { path });
        addLog(`${fileName(path)}: synced ${count} entries`, true);
      } catch (err) {
        addLog(`${fileName(path)}: ${err}`, false);
      } finally {
        syncBtn.disabled = false;
      }
    });

    const removeBtn = document.createElement("button");
    removeBtn.type = "button";
    removeBtn.className = "ghost danger";
    removeBtn.textContent = "Remove";
    removeBtn.addEventListener("click", async () => {
      const config = await invoke("remove_file", { path });
      renderFiles(config.files);
    });

    actions.append(syncBtn, removeBtn);
    li.append(info, actions);
    els.fileList.appendChild(li);
  }
}

function addLog(message, ok) {
  const empty = els.log.querySelector(".empty");
  if (empty) empty.remove();

  const li = document.createElement("li");
  li.className = `log-item ${ok ? "ok" : "fail"}`;
  const time = new Date().toLocaleTimeString();
  li.innerHTML = `<span class="log-time">${time}</span><span class="log-msg"></span>`;
  li.querySelector(".log-msg").textContent = message;
  els.log.prepend(li);

  while (els.log.children.length > 50) els.log.lastChild.remove();
}

async function saveApiKey(value) {
  try {
    await invoke("set_api_key", { apiKey: value });
    setKeyStatus("Saved", "ok");
  } catch (err) {
    setKeyStatus(String(err), "fail");
  }
}

async function init() {
  els.apiKey = document.querySelector("#api-key");
  els.toggleKey = document.querySelector("#toggle-key");
  els.keyStatus = document.querySelector("#key-status");
  els.addFiles = document.querySelector("#add-files");
  els.fileList = document.querySelector("#file-list");
  els.log = document.querySelector("#log");

  const config = await invoke("get_config");

  els.apiKey.value = config.api_key || "";
  renderFiles(config.files || []);

  // Debounced auto-save of the API key as the user types.
  els.apiKey.addEventListener("input", () => {
    clearTimeout(saveTimer);
    setKeyStatus("Saving…");
    saveTimer = setTimeout(() => saveApiKey(els.apiKey.value), 500);
  });

  els.toggleKey.addEventListener("click", () => {
    els.apiKey.type = els.apiKey.type === "password" ? "text" : "password";
  });

  els.addFiles.addEventListener("click", async () => {
    els.addFiles.disabled = true;
    try {
      const { config, skipped } = await invoke("add_files");
      renderFiles(config.files || []);
      if (skipped > 0) {
        const noun = skipped === 1 ? "file" : "files";
        addLog(`${skipped} ${noun} ignored (not *_MapData.sav)`, false);
      }
    } catch (err) {
      addLog(`Could not add files: ${err}`, false);
    } finally {
      els.addFiles.disabled = false;
    }
  });

  // Live updates pushed from the Rust watcher.
  await listen("sync-status", (event) => {
    const { file, ok, message } = event.payload;
    addLog(`${file}: ${message}`, ok);
  });
}

window.addEventListener("DOMContentLoaded", init);
