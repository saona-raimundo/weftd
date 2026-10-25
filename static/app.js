// static/app.js

const messagesEl = document.getElementById("messages");
const inputEl = document.getElementById("input");
const errorBanner = document.getElementById("error-banner");
const whisperList = document.getElementById("whisper-list");

const DEBUG = new URLSearchParams(location.search).has("debug");
if (DEBUG) {
  const log = document.getElementById("event-log");
  if (log) log.classList.add("visible");
}

function debugState() {
  if (!DEBUG) return;
  document.getElementById("debug-hint").innerText =
    `ws=${ws?.readyState} pending=${pendingSubmitId?.slice(0, 4) ?? "-"} ` +
    `outbox=${outbox.length}`;
}

function logEvent(direction, data) {
  if (!DEBUG) return;
  const log = document.getElementById("event-log");
  const line = document.createElement("div");
  line.innerText = `${direction} ${JSON.stringify(data).slice(0, 80)}`;
  log.appendChild(line);
  while (log.children.length > 10) log.removeChild(log.firstChild);
  log.scrollTop = log.scrollHeight;
}

const messages = [];
let waiting = false;
let cueEntries = [];
let activeWhispers = [];
let summaryState = { text: "", up_to: 0 };
let endWrappers = [];
let endWrapperRender = 0;
let samplerFields = [];
let autoUpdateSummary = false;

let ws = null;
const outbox = [];

let pendingSubmitId = null;
let pendingTimeoutId = null;
const PENDING_TIMEOUT_MS = 5000;

function connect() {
  if (
    ws &&
    (ws.readyState === WebSocket.OPEN || ws.readyState === WebSocket.CONNECTING)
  )
    return;
  const proto = location.protocol === "https:" ? "wss:" : "ws:";
  ws = new WebSocket(`${proto}//${location.host}/ws`);
  ws.onopen = handleOpen;
  ws.onmessage = handleMessage;
  ws.onclose = () => debugState();
  ws.onerror = () => debugState();
}

function send(payload) {
  logEvent("→", payload);
  if (ws?.readyState === WebSocket.OPEN) {
    ws.send(JSON.stringify(payload));
  } else {
    outbox.push(payload);
    connect();
  }
  debugState();
}

function newSubmitId() {
  if (crypto?.randomUUID) return crypto.randomUUID();
  return "id-" + Date.now() + "-" + Math.random().toString(36).slice(2, 10);
}

function setSendDisabled(disabled) {
  document.getElementById("send-btn").disabled = disabled;
}

function clearPending(success) {
  if (pendingTimeoutId) {
    clearTimeout(pendingTimeoutId);
    pendingTimeoutId = null;
  }
  pendingSubmitId = null;
  setSendDisabled(false);
  if (success) {
    inputEl.value = "";
    inputEl.style.height = "3rem";
  }
  debugState();
}

function onPendingTimeout() {
  pendingTimeoutId = null;
  pendingSubmitId = null;
  setSendDisabled(false);
  showError("Send timed out — try again.");
  debugState();
}

function handleOpen() {
  while (outbox.length) ws.send(JSON.stringify(outbox.shift()));
  ws.send(JSON.stringify({ action: "submit", role: "user", content: "/sync" }));
  debugState();
}

function handleMessage(e) {
  const data = JSON.parse(e.data);
  logEvent("←", data);
  switch (data.event) {
    case "thinking":
      waiting = true;
      document.getElementById("hint").innerText = "Waiting for response...";
      addPlaceholder("Typing...");
      break;
    case "error":
      waiting = false;
      document.getElementById("hint").innerText =
        "Alt+Enter or Ctrl+Enter to send";
      removePlaceholder();
      showError(data.content);
      setWrapperControlsDisabled(false);
      if (pendingSubmitId !== null && data.submit_id === pendingSubmitId) {
        clearPending(false);
      }
      break;
    case "summarizing":
      document.getElementById("hint").innerText = "Summarizing...";
      break;
    case "summarized":
      document.getElementById("hint").innerText =
        "Alt+Enter or Ctrl+Enter to send";
      break;
    case "summary":
      summaryState = { text: data.text, up_to: data.up_to };
      renderSummary();
      document.getElementById("summary-banner").classList.remove("visible");
      break;
    case "summary_needed":
      if (autoUpdateSummary) {
        send({ action: "summary_trigger" });
      } else {
        const banner = document.getElementById("summary-banner");
        banner.innerText = `Summary needed — ${Math.round(data.ratio * 100)}% of context used`;
        banner.classList.add("visible");
      }
      break;
    case "sync":
      waiting = false;
      document.getElementById("hint").innerText =
        "Alt+Enter or Ctrl+Enter to send";
      removePlaceholder();
      messages.length = 0;
      messages.push(...data.messages);
      renderAllMessages();
      summaryState = { text: data.summary.text, up_to: data.summary.up_to };
      renderSummary();
      endWrappers = data.end_wrappers ?? [];
      renderEndWrappers();
      samplerFields = data.sampler ?? [];
      renderSampler();
      if (pendingSubmitId !== null && data.submit_id === pendingSubmitId) {
        clearPending(true);
      }
      break;
    case "token":
      updatePlaceholder(data.content);
      break;
    case "whispers":
      renderWhispers(data.active);
      break;
    case "cues":
      cueEntries = data.entries;
      renderCues();
      break;
    case "ack":
      if (pendingSubmitId !== null && data.submit_id === pendingSubmitId) {
        clearPending(true);
      }
      break;
  }
  debugState();
}

function submit() {
  logEvent("tap", { waiting, pending: pendingSubmitId?.slice(0, 4) ?? null });
  if (waiting) {
    logEvent("blocked", "waiting");
    return;
  }
  if (pendingSubmitId !== null) {
    logEvent("blocked", "pending=" + pendingSubmitId.slice(0, 4));
    return;
  }

  const raw = inputEl.value;
  logEvent("input", {
    type: typeof raw,
    len: raw?.length ?? -1,
    val: String(raw).slice(0, 20),
  });

  const text = raw?.trim() ?? "";
  if (!text) {
    logEvent("blocked", "empty");
    return;
  }

  pendingSubmitId = newSubmitId();
  setSendDisabled(true);
  debugState();
  pendingTimeoutId = setTimeout(onPendingTimeout, PENDING_TIMEOUT_MS);
  send({
    action: "submit",
    role: "user",
    content: text,
    submit_id: pendingSubmitId,
  });
  // input value NOT cleared — only cleared once sync confirms
}

inputEl.addEventListener("keydown", (e) => {
  if ((e.key === "Enter" && e.altKey) || (e.key === "Enter" && e.ctrlKey)) {
    e.preventDefault();
    submit();
  }
});

document.getElementById("send-btn").addEventListener("click", submit);

inputEl.addEventListener("input", () => {
  const scrollBottom =
    messagesEl.scrollHeight - messagesEl.scrollTop - messagesEl.clientHeight;
  inputEl.style.height = "3rem";
  inputEl.style.height = Math.min(inputEl.scrollHeight, 160) + "px";
  messagesEl.scrollTop =
    messagesEl.scrollHeight - messagesEl.clientHeight - scrollBottom;
});

// Action buttons
document.querySelectorAll("#actions button[data-cmd]").forEach((btn) => {
  btn.addEventListener("click", () => {
    if (waiting || pendingSubmitId !== null) return;
    send({ action: "submit", role: "user", content: btn.dataset.cmd });
  });
});

document.querySelectorAll("#actions button[data-prefix]").forEach((btn) => {
  btn.addEventListener("click", () => {
    if (waiting || pendingSubmitId !== null) return;
    inputEl.value = btn.dataset.prefix;
    inputEl.focus();
  });
});

function renderMessage(index) {
  const msg = messages[index];
  const div = document.createElement("div");
  div.className = "message " + msg.role;

  const content = document.createElement("div");
  content.className = "msg-content";
  content.innerText = msg.content;
  content.contentEditable = "true";

  content.addEventListener("blur", () => {
    const newContent = content.innerText;
    if (newContent !== messages[index].content) {
      messages[index].content = newContent;
      send({ action: "edit", index, content: newContent });
    }
  });

  const deleteBtn = document.createElement("button");
  deleteBtn.className = "msg-delete";
  deleteBtn.innerText = "×";
  deleteBtn.addEventListener("click", (e) => {
    e.stopPropagation();
    if (waiting || pendingSubmitId !== null) return;
    send({ action: "submit", role: "user", content: `/delete ${index}` });
  });

  div.appendChild(deleteBtn);
  div.appendChild(content);
  messagesEl.appendChild(div);
}

document.getElementById("summary-text").addEventListener("blur", () => {
  const text = document.getElementById("summary-text").value;
  if (text !== summaryState.text) {
    const up_to =
      parseInt(document.getElementById("summary-up-to").value, 10) || 0;
    send({ action: "summary_edit", text, up_to });
  }
});

document.getElementById("summary-up-to").addEventListener("blur", () => {
  const up_to =
    parseInt(document.getElementById("summary-up-to").value, 10) || 0;
  if (up_to !== summaryState.up_to) {
    const text = document.getElementById("summary-text").value;
    send({ action: "summary_edit", text, up_to });
  }
});

document.getElementById("summary-gen-btn").addEventListener("click", () => {
  send({ action: "summary_trigger" });
});

document.getElementById("summary-auto").addEventListener("change", (e) => {
  autoUpdateSummary = e.target.checked;
});

document.getElementById("wrapper-add-btn").addEventListener("click", () => {
  if (waiting || pendingSubmitId !== null) return;
  const text = prompt("New end wrapper:");
  if (text === null) return;
  if (text.trim() === "") {
    showError("End wrapper can't be empty");
    return;
  }
  setWrapperControlsDisabled(true);
  send({
    action: "submit",
    role: "user",
    content: `/wrapper end add ${endWrappers.length} ${text}`,
  });
});

function renderAllMessages() {
  messagesEl.innerHTML = "";
  messages.forEach((_, i) => renderMessage(i));
  messagesEl.scrollTop = messagesEl.scrollHeight;
}

function addPlaceholder(text) {
  const div = document.createElement("div");
  div.className = "message assistant placeholder";
  div.innerText = text;
  div.id = "placeholder";
  messagesEl.appendChild(div);
  messagesEl.scrollTop = messagesEl.scrollHeight;
}

function updatePlaceholder(text) {
  const el = document.getElementById("placeholder");
  if (el) {
    el.innerText = text;
  }
}

function removePlaceholder() {
  const el = document.getElementById("placeholder");
  if (el) el.remove();
}

function showError(text) {
  errorBanner.innerText = text;
  errorBanner.classList.add("visible");
}

function renderWhispers(active) {
  activeWhispers = active;
  whisperList.innerHTML = "";
  active.forEach((w, i) => {
    const div = document.createElement("div");
    div.className = "whisper";

    const span = document.createElement("span");
    span.innerText = `"${w.text}" (${w.turns_remaining} turn${w.turns_remaining === 1 ? "" : "s"} left)`;

    const btn = document.createElement("button");
    btn.innerText = "×";
    btn.addEventListener("click", () => {
      if (waiting || pendingSubmitId !== null) return;
      send({ action: "submit", role: "user", content: `/cancel ${i}` });
    });

    div.appendChild(span);
    div.appendChild(btn);
    whisperList.appendChild(div);
  });
  renderCues();
}

function renderCues() {
  const container = document.getElementById("cues");
  container.innerHTML = "";
  const activeTexts = new Set(activeWhispers.map((w) => w.text));
  cueEntries.forEach((entry) => {
    const btn = document.createElement("button");
    btn.className = "cue-button";
    const isActive = activeTexts.has(entry.content);
    if (isActive) btn.classList.add("active");
    btn.textContent = entry.label;
    btn.addEventListener("click", () => {
      if (isActive || waiting || pendingSubmitId !== null) return;
      const cmd =
        entry.turns === 1
          ? `/whisper ${entry.content}`
          : `/whisper ${entry.turns} ${entry.content}`;
      send({ action: "submit", role: "user", content: cmd });
    });
    container.appendChild(btn);
  });
}

function renderSummary() {
  const textarea = document.getElementById("summary-text");
  const upTo = document.getElementById("summary-up-to");
  if (textarea && document.activeElement !== textarea) {
    textarea.value = summaryState.text;
  }
  if (upTo && document.activeElement !== upTo) {
    upTo.value = summaryState.up_to;
  }
}

// Add/remove shift indices, so wrapper controls stay locked from a click
// until the next sync (or error) reflects it.
function setWrapperControlsDisabled(disabled) {
  document
    .querySelectorAll(".wrapper-remove, #wrapper-add-btn")
    .forEach((el) => (el.disabled = disabled));
}

function renderEndWrappers() {
  setWrapperControlsDisabled(false);
  const list = document.getElementById("wrapper-list");
  const existing = list.querySelectorAll("textarea");
  // Same count: update in place so a focused textarea keeps its caret.
  if (existing.length === endWrappers.length && existing.length > 0) {
    existing.forEach((textarea, i) => {
      if (document.activeElement !== textarea) {
        textarea.value = endWrappers[i].content;
      }
    });
    return;
  }
  // A rebuild can blur a focused textarea; its index is stale by then.
  const render = ++endWrapperRender;
  list.innerHTML = "";
  if (endWrappers.length === 0) {
    list.innerText = "No end wrappers in this session.";
    return;
  }
  endWrappers.forEach((wrapper, i) => {
    const item = document.createElement("div");
    item.className = "wrapper-item";

    const textarea = document.createElement("textarea");
    textarea.className = "wrapper-text";
    textarea.value = wrapper.content;
    textarea.addEventListener("blur", () => {
      if (render !== endWrapperRender) return;
      const text = textarea.value;
      if (text === endWrappers[i].content) return;
      if (text.trim() === "") {
        textarea.value = endWrappers[i].content;
        showError("End wrapper can't be empty");
        return;
      }
      send({
        action: "submit",
        role: "user",
        content: `/wrapper end edit ${i} ${text}`,
      });
    });

    const removeBtn = document.createElement("button");
    removeBtn.className = "wrapper-remove";
    removeBtn.innerText = "×";
    removeBtn.title = "Remove end wrapper";
    removeBtn.addEventListener("click", () => {
      if (waiting || pendingSubmitId !== null) return;
      setWrapperControlsDisabled(true);
      send({
        action: "submit",
        role: "user",
        content: `/wrapper end remove ${i}`,
      });
    });

    item.appendChild(removeBtn);
    item.appendChild(textarea);
    list.appendChild(item);
  });
}

// Empty input = unset (server default); otherwise the value is JSON.
function samplerText(value) {
  return value === null || value === undefined ? "" : JSON.stringify(value);
}

function renderSampler() {
  const list = document.getElementById("sampler-list");
  // The field set is fixed: build once, then update values in place.
  if (list.querySelectorAll("input").length !== samplerFields.length) {
    list.innerHTML = "";
    samplerFields.forEach((field) => {
      const input = document.createElement("input");
      input.className = "sampler-value";
      input.id = `sampler-${field.key}`;
      input.dataset.key = field.key;
      input.placeholder = "server default";
      // Values are JSON: keep iOS from inserting curly quotes or capitals.
      input.spellcheck = false;
      input.autocomplete = "off";
      input.setAttribute("autocapitalize", "off");
      input.setAttribute("autocorrect", "off");
      input.addEventListener("blur", () => onSamplerBlur(input));

      const label = document.createElement("label");
      label.className = "sampler-key";
      label.htmlFor = input.id;
      label.innerText = field.key;

      list.append(label, input);
    });
  }
  list.querySelectorAll("input").forEach((input, i) => {
    const field = samplerFields[i];
    input.disabled = !field.editable;
    input.title = field.editable ? "" : "Read-only here; change it in the TOML";
    if (document.activeElement !== input) {
      input.value = samplerText(field.value);
    }
  });
}

function onSamplerBlur(input) {
  const field = samplerFields.find((f) => f.key === input.dataset.key);
  if (!field || !field.editable) return;
  const text = input.value.trim();
  if (text === samplerText(field.value)) return;
  send({
    action: "submit",
    role: "user",
    content:
      text === ""
        ? `/sampler unset ${field.key}`
        : `/sampler set ${field.key} ${text}`,
  });
}

connect();
