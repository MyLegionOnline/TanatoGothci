// TanatoGothci frontend shell (placeholder for the full two-screen UI).
// Talks to the Rust backend through the Tauri `window.__TAURI__` global.
// The full Settings / Reasoning / ASCII experience is built out in the
// frontend task; here we wire the scaffold commands so the app boots and the
// core command surface is exercised.

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const $ = (id) => document.getElementById(id);

function switchTab(name) {
  document.querySelectorAll(".tab").forEach((t) =>
    t.classList.toggle("active", t.dataset.tab === name)
  );
  document.querySelectorAll(".screen").forEach((s) =>
    s.classList.toggle("active", s.id === "screen-" + name)
  );
}

// Both screens (reasoning + ASCII) are always visible in the session layout,
// so "reasoning"/"ascii" tab names from the old three-tab layout map onto the
// single "session" screen. Keeps tab switching logic minimal.
function normalizeTab(name) {
  return name === "reasoning" || name === "ascii" ? "session" : name;
}

function append(lineEl, text, cls) {
  // Use <span> (phrasing content) instead of <div> (block) — a <div> is
  // invalid inside <pre> and breaks its preformatted line layout.
  const s = document.createElement("span");
  s.textContent = text;
  if (cls) s.className = cls;
  lineEl.appendChild(s);
  // Preserve one line-break per appended chunk inside the <pre>.
  lineEl.appendChild(document.createTextNode("\n"));
  lineEl.scrollTop = lineEl.scrollHeight;
}

async function main() {
  document.querySelectorAll(".tab").forEach((t) =>
    t.addEventListener("click", () => switchTab(normalizeTab(t.dataset.tab)))
  );

  // Load saved config into the settings form.
  try {
    const cfg = await invoke("get_config");
    $("cfg-base-url").value = cfg.base_url || "";
    $("cfg-api-key").value = cfg.api_key || "";
    $("cfg-model").value = cfg.model || "";
    $("cfg-artifacts").value = cfg.artifacts_dir || "";
    $("cfg-reasoning").checked = !!cfg.reasoning_enabled;
    $("cfg-rlevel").value = cfg.reasoning_level || 3;
    $("cfg-binterval").value = cfg.battery_interval_secs || 40;
  } catch (e) {
    console.error("get_config failed", e);
  }

  async function saveSettings() {
    await invoke("save_config", {
      settings: {
        base_url: $("cfg-base-url").value,
        api_key: $("cfg-api-key").value,
        model: $("cfg-model").value,
        artifacts_dir: $("cfg-artifacts").value,
        use_system_prompt: true,
        reasoning_enabled: $("cfg-reasoning").checked,
        reasoning_level: Number($("cfg-rlevel").value) || 3,
        battery_interval_secs: Number($("cfg-binterval").value) || 40,
      },
    });
  }

  $("btn-validate").addEventListener("click", async () => {
    await saveSettings();
    const box = $("validation-result");
    box.classList.remove("hidden", "ok", "err");
    box.textContent = "Validating…";
    try {
      const r = await invoke("validate_connection");
      box.textContent = r.ok ? `Connection OK (${r.latency_ms} ms)` : "Failed: " + r.error;
      box.classList.add(r.ok ? "ok" : "err");
    } catch (e) {
      box.textContent = "Failed: " + e;
      box.classList.add("err");
    }
  });

  $("btn-start").addEventListener("click", async () => {
    try {
      await saveSettings();
      await invoke("start_run");
      $("btn-start").disabled = true;
      $("btn-stop").disabled = false;
      $("reasoning-log").textContent = "";
      $("ascii-art").textContent = "";
    } catch (e) {
      alert("Start failed: " + e);
    }
  });

  $("btn-stop").addEventListener("click", async () => {
    try { await invoke("stop_run"); } catch (e) { console.error(e); }
  });

  // Live events from the backend.
  await listen("battery:level", (e) => {
    const { level } = e.payload;
    const b = $("battery");
    b.textContent = level + "%";
    b.classList.toggle("critical", level < 10);
    append($("reasoning-log"), `[battery ${level}%]`, "cmd-line");
  });

  await listen("reasoning:delta", (e) => {
    const { text, kind } = e.payload;
    append($("reasoning-log"), text, kind === "command" ? "cmd-line" : "");
  });

  await listen("command:detected", (e) => {
    append($("reasoning-log"), "COMMAND: " + e.payload.cmd, "cmd-line");
  });

  await listen("ascii:frame", (e) => {
    $("ascii-art").textContent = e.payload.text;
  });

  await listen("session:end", () => {
    $("btn-start").disabled = false;
    $("btn-stop").disabled = true;
  });
}

main();
