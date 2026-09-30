/*!
 * Aerini embeddable chat widget.
 *
 * Usage:
 *   <script
 *     src="https://YOUR_SERVER/aerini-widget.js"
 *     data-server="https://YOUR_SERVER"
 *     data-workflow-id="wf_abc123"
 *     data-secret="the Webhook node's configured secret"
 *     data-token="a read-scoped, single-workflow-ACL'd API token (see /api/tokens)"
 *     data-session-id="optional — generated and persisted in localStorage if omitted"
 *     data-allow-images="true"
 *     data-max-length="2000"
 *     data-theme="light"
 *   ></script>
 *
 * Two secrets, two different jobs (both are visible in page source by
 * construction — same exposure the Webhook node's own config description
 * already accepts):
 *   - data-secret: fires the workflow's Webhook trigger via the server's
 *     /api/widget/:workflow_id/trigger relay (see aerini-server's
 *     routes/widget.rs for why a relay is needed instead of POSTing the
 *     browser straight at the webhook port).
 *   - data-token: a *read*-scoped API token, ACL'd to this one workflow
 *     (POST /api/tokens then POST /api/tokens/:id/workflows/:workflow_id),
 *     used only to read this workflow's reply over /api/events (SSE).
 *
 * Message flow does NOT use the trigger POST's own HTTP response — the
 * workflow runs in the background and the reply arrives later as a
 * `scheduler-status` SSE event. See the matching doc comment at the top of
 * the desktop app's src/panels/ChatPanel.ts for the full discrepancy
 * writeup against the webhook node's actual (always-"OK") behavior.
 *
 * Zero external dependencies, no build step. Markdown rendering is
 * deliberately NOT implemented here (unlike the desktop Chat Panel, which
 * uses `marked` + `DOMPurify`) — pulling in a markdown lib would break the
 * zero-dependency, single-file, no-build constraint this widget is built
 * under. Replies render as plain, HTML-escaped text with line breaks
 * preserved; this is a deliberate scope reduction, not a dropped bug.
 */
(function () {
  "use strict";

  var scriptEl =
    document.currentScript ||
    (function () {
      var scripts = document.getElementsByTagName("script");
      return scripts[scripts.length - 1];
    })();

  function attr(name, fallback) {
    var v = scriptEl.getAttribute(name);
    return v === null || v === "" ? fallback : v;
  }
  function attrBool(name, fallback) {
    var v = scriptEl.getAttribute(name);
    if (v === null || v === "") return fallback;
    return v === "true" || v === "1";
  }
  function attrInt(name, fallback) {
    var v = scriptEl.getAttribute(name);
    var n = v === null ? NaN : parseInt(v, 10);
    return isFinite(n) && n > 0 ? n : fallback;
  }

  var SERVER = (attr("data-server", "") || "").replace(/\/+$/, "");
  var WORKFLOW_ID = attr("data-workflow-id", "");
  var SECRET = attr("data-secret", "");
  var TOKEN = attr("data-token", "");
  var MAX_LENGTH = attrInt("data-max-length", 2000);
  var THEME = attr("data-theme", "light") === "dark" ? "dark" : "light";
  var ALLOW_IMAGES = attrBool("data-allow-images", true);

  if (!SERVER || !WORKFLOW_ID || !SECRET) {
    console.error(
      "[aerini-widget] data-server, data-workflow-id, and data-secret are all required — widget not started."
    );
    return;
  }

  function toHex2(n) {
    var s = n.toString(16);
    return s.length === 1 ? "0" + s : s;
  }

  function makeUuid() {
    if (window.crypto && typeof window.crypto.randomUUID === "function") {
      return window.crypto.randomUUID();
    }
    if (window.crypto && typeof window.crypto.getRandomValues === "function") {
      var bytes = new Uint8Array(16);
      window.crypto.getRandomValues(bytes);
      bytes[6] = (bytes[6] & 0x0f) | 0x40;
      bytes[8] = (bytes[8] & 0x3f) | 0x80;
      var hex = "";
      for (var i = 0; i < 16; i++) {
        hex += toHex2(bytes[i]);
        if (i === 3 || i === 5 || i === 7 || i === 9) hex += "-";
      }
      return hex;
    }
    // The session id is what keeps one visitor's replies from being delivered to
    // another, so it must be unguessable; never fall back to Math.random().
    return null;
  }

  var SESSION_ID = attr("data-session-id", "") || (function () {
    var key = "aerini_widget_session_" + WORKFLOW_ID;
    try {
      var existing = window.localStorage ? window.localStorage.getItem(key) : null;
      if (existing) return existing;
      var id = makeUuid();
      if (id && window.localStorage) window.localStorage.setItem(key, id);
      return id || "";
    } catch (e) {
      // Private browsing / storage disabled — fall back to a per-load id.
      return makeUuid() || "";
    }
  })();

  if (!SESSION_ID) {
    console.error(
      "[aerini-widget] no Web Crypto API available to generate a session id — widget not started. Set data-session-id to supply one."
    );
    return;
  }

  var outputNodeId = null;
  var panelOpen = false;
  var awaitingReply = false;
  var pendingTimer = null;
  var RESPONSE_TIMEOUT_MS = 30000;
  var SSE_RECONNECT_DELAY_MS = 3000;

  // ---- scoped styles (prefixed so they cannot collide with the host page) ----
  var P = "aerini-w-";
  var bg = THEME === "dark" ? "#1f2125" : "#fff";
  var fg = THEME === "dark" ? "#f0f0f0" : "#111";
  var hdrBg = THEME === "dark" ? "#2a2d33" : "#4f46e5";
  var aiBubbleBg = THEME === "dark" ? "#2a2d33" : "#f1f1f3";
  var borderCol = THEME === "dark" ? "#3a3d44" : "#eee";
  var inputBg = THEME === "dark" ? "#16171a" : "#fff";

  var css =
    "." + P + "bubble{position:fixed;bottom:20px;right:20px;width:56px;height:56px;border-radius:50%;" +
    "background:#4f46e5;color:#fff;border:none;cursor:pointer;box-shadow:0 4px 14px rgba(0,0,0,.25);" +
    "font-size:24px;z-index:2147483000;display:flex;align-items:center;justify-content:center;padding:0;}" +
    "." + P + "bubble:hover{filter:brightness(1.08);}" +
    "." + P + "win{position:fixed;bottom:88px;right:20px;width:340px;max-width:calc(100vw - 40px);" +
    "height:480px;max-height:calc(100vh - 120px);background:" + bg + ";color:" + fg + ";" +
    "border-radius:12px;box-shadow:0 8px 30px rgba(0,0,0,.3);display:none;flex-direction:column;" +
    "overflow:hidden;z-index:2147483000;font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',Roboto,sans-serif;" +
    "font-size:14px;}" +
    "." + P + "win.open{display:flex;}" +
    "." + P + "hdr{padding:12px 14px;background:" + hdrBg + ";color:#fff;display:flex;" +
    "justify-content:space-between;align-items:center;flex:0 0 auto;}" +
    "." + P + "hdr span{font-weight:600;}" +
    "." + P + "hdr button{background:none;border:none;color:#fff;font-size:20px;line-height:1;cursor:pointer;padding:0 2px;}" +
    "." + P + "msgs{flex:1 1 auto;overflow-y:auto;padding:12px;display:flex;flex-direction:column;gap:8px;}" +
    "." + P + "bub{max-width:80%;padding:8px 12px;border-radius:14px;white-space:pre-wrap;word-break:break-word;line-height:1.4;}" +
    "." + P + "bub.user{align-self:flex-end;background:#4f46e5;color:#fff;border-bottom-right-radius:4px;}" +
    "." + P + "bub.ai{align-self:flex-start;background:" + aiBubbleBg + ";border-bottom-left-radius:4px;}" +
    "." + P + "bub.error{align-self:flex-start;background:#fde8e8;color:#9b1c1c;border-bottom-left-radius:4px;}" +
    "." + P + "bub img{max-width:100%;max-height:220px;border-radius:8px;display:block;margin-top:4px;object-fit:contain;}" +
    "." + P + "typing{align-self:flex-start;display:flex;gap:4px;padding:8px 12px;}" +
    "." + P + "typing span{width:6px;height:6px;border-radius:50%;background:#999;animation:" + P + "blink 1.2s infinite;}" +
    "." + P + "typing span:nth-child(2){animation-delay:.2s;}" +
    "." + P + "typing span:nth-child(3){animation-delay:.4s;}" +
    "@keyframes " + P + "blink{0%,80%,100%{opacity:.3;}40%{opacity:1;}}" +
    "." + P + "inrow{display:flex;gap:6px;padding:10px;border-top:1px solid " + borderCol + ";flex:0 0 auto;}" +
    "." + P + "inrow textarea{flex:1;resize:none;border:1px solid " + borderCol + ";border-radius:8px;" +
    "padding:8px;font:inherit;background:" + inputBg + ";color:inherit;max-height:90px;}" +
    "." + P + "inrow button{border:none;background:#4f46e5;color:#fff;border-radius:8px;padding:0 14px;" +
    "cursor:pointer;font-weight:600;}" +
    "." + P + "inrow button:disabled{opacity:.5;cursor:default;}" +
    "." + P + "banner{padding:6px 12px;font-size:12px;text-align:center;color:" + (THEME === "dark" ? "#aaa" : "#666") + ";flex:0 0 auto;}";

  var styleEl = document.createElement("style");
  styleEl.textContent = css;
  document.head.appendChild(styleEl);

  // ---- DOM ----
  var bubble = document.createElement("button");
  bubble.type = "button";
  bubble.className = P + "bubble";
  bubble.setAttribute("aria-label", "Open chat");
  bubble.textContent = "\uD83D\uDCAC"; // 💬

  var win = document.createElement("div");
  win.className = P + "win";
  win.innerHTML =
    '<div class="' + P + 'hdr"><span>Chat</span><button type="button" aria-label="Close chat">\u00D7</button></div>' +
    '<div class="' + P + 'msgs"></div>' +
    '<div class="' + P + 'banner" style="display:none;"></div>' +
    '<div class="' + P + 'inrow">' +
    '<textarea rows="1" maxlength="' + MAX_LENGTH + '" placeholder="Type a message..."></textarea>' +
    '<button type="button">Send</button>' +
    "</div>";

  document.body.appendChild(bubble);
  document.body.appendChild(win);

  var msgsEl = win.querySelector("." + P + "msgs");
  var bannerEl = win.querySelector("." + P + "banner");
  var closeBtn = win.querySelector("." + P + "hdr button");
  var inputEl = win.querySelector("textarea");
  var sendBtn = win.querySelector("." + P + "inrow button");

  function toggle() {
    panelOpen = !panelOpen;
    win.classList.toggle("open", panelOpen);
    if (panelOpen) inputEl.focus();
  }
  bubble.addEventListener("click", toggle);
  closeBtn.addEventListener("click", toggle);

  function escapeHtml(s) {
    return String(s).replace(/[&<>"']/g, function (c) {
      return { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c];
    });
  }

  function appendBubble(role, text) {
    var b = document.createElement("div");
    b.className = P + "bub " + role;
    b.innerHTML = escapeHtml(text == null ? "" : String(text));
    msgsEl.appendChild(b);
    msgsEl.scrollTop = msgsEl.scrollHeight;
    return b;
  }

  function appendImages(files) {
    var b = document.createElement("div");
    b.className = P + "bub ai";
    files.forEach(function (f) {
      var img = document.createElement("img");
      img.src = "data:" + f.mime_type + ";base64," + f.data;
      img.alt = f.filename || "image";
      b.appendChild(img);
    });
    msgsEl.appendChild(b);
    msgsEl.scrollTop = msgsEl.scrollHeight;
  }

  var typingEl = null;
  function showTyping() {
    removeTyping();
    typingEl = document.createElement("div");
    typingEl.className = P + "typing";
    typingEl.innerHTML = "<span></span><span></span><span></span>";
    msgsEl.appendChild(typingEl);
    msgsEl.scrollTop = msgsEl.scrollHeight;
  }
  function removeTyping() {
    if (typingEl && typingEl.parentNode) typingEl.parentNode.removeChild(typingEl);
    typingEl = null;
  }

  function setAwaiting(v) {
    awaitingReply = v;
    inputEl.disabled = v;
    sendBtn.disabled = v;
  }

  function clearPendingTimer() {
    if (pendingTimer) {
      clearTimeout(pendingTimer);
      pendingTimer = null;
    }
  }

  function failPending(message) {
    clearPendingTimer();
    setAwaiting(false);
    removeTyping();
    appendBubble("error", message);
  }

  function isFilesContainer(value) {
    return !!value && typeof value === "object" && Array.isArray(value.files);
  }
  function isImageFile(f) {
    return (
      !!f &&
      typeof f === "object" &&
      typeof f.data === "string" &&
      typeof f.mime_type === "string" &&
      f.mime_type.indexOf("image/") === 0
    );
  }

  // Mirrors the desktop Chat Panel's renderAiReply() — same shape:
  // { output_type?: "media_batch", value: ... } — see ChatPanel.ts.
  function renderReply(raw) {
    removeTyping();
    if (raw === undefined || raw === null) {
      appendBubble("ai", "Workflow completed but the Output node produced nothing.");
      return;
    }
    var isMediaBatch = raw["output_type"] === "media_batch";
    var value = raw["value"];

    if (isMediaBatch && ALLOW_IMAGES && isFilesContainer(value)) {
      var images = value.files.filter(isImageFile);
      if (images.length > 0) {
        appendImages(images);
        return;
      }
    }

    var text;
    if (typeof value === "string") text = value;
    else if (value === undefined || value === null) text = JSON.stringify(raw, null, 2);
    else text = JSON.stringify(value, null, 2);
    appendBubble("ai", text);
  }

  function sendMessage() {
    var text = inputEl.value.trim();
    if (!text || awaitingReply) return;
    if (text.length > MAX_LENGTH) {
      appendBubble("error", "Message exceeds the " + MAX_LENGTH + "-character limit.");
      return;
    }

    inputEl.value = "";
    inputEl.style.height = "auto";
    appendBubble("user", text);
    setAwaiting(true);
    showTyping();

    fetch(SERVER + "/api/widget/" + encodeURIComponent(WORKFLOW_ID) + "/trigger", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({
        secret: SECRET,
        body: { message: text, session_id: SESSION_ID },
      }),
    })
      .then(function (res) {
        if (!res.ok) {
          return res
            .json()
            .catch(function () {
              return {};
            })
            .then(function (j) {
              var msg = j.error || "Request failed (" + res.status + ")";
              var retryAfter = res.status === 429 ? res.headers.get("Retry-After") : null;
              if (retryAfter) msg += " (try again in " + retryAfter + "s)";
              throw new Error(msg);
            });
        }
        return res.json();
      })
      .then(function (j) {
        if (j && j.output_node_id) outputNodeId = j.output_node_id;
        clearPendingTimer();
        pendingTimer = setTimeout(function () {
          failPending("No response after 30s. The workflow may be busy or the message was dropped.");
        }, RESPONSE_TIMEOUT_MS);
      })
      .catch(function (err) {
        failPending(err && err.message ? err.message : "Could not reach the workflow.");
      });
  }

  sendBtn.addEventListener("click", sendMessage);
  inputEl.addEventListener("keydown", function (e) {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      sendMessage();
    }
  });
  inputEl.addEventListener("input", function () {
    inputEl.style.height = "auto";
    inputEl.style.height = Math.min(inputEl.scrollHeight, 90) + "px";
  });

  // ---- SSE: manual fetch + ReadableStream reader ----
  // EventSource cannot set an Authorization header, so the reply stream is
  // read by hand instead. Connects eagerly at load (not on first panel
  // open) so a message sent immediately after opening the panel can never
  // race the subscription — tokio::sync::broadcast (server side) only
  // delivers events to receivers that already exist at emit time; a late
  // subscription would silently miss a fast reply.
  var sseAbort = null;

  function scheduleSseReconnect() {
    setTimeout(connectSse, SSE_RECONNECT_DELAY_MS);
  }

  function connectSse() {
    if (!TOKEN) {
      bannerEl.style.display = "block";
      bannerEl.textContent = "Chat replies are unavailable (no API token configured).";
      return;
    }
    if (sseAbort) sseAbort.abort();
    sseAbort = new AbortController();

    // session_id scopes the stream server-side to just this visitor's own
    // replies — without it, every visitor sharing this page's data-token
    // would receive every other visitor's messages too (the token is
    // workflow-scoped, not visitor-scoped; see widget-embedding.md).
    var url = SERVER + "/api/events?workflow_id=" + encodeURIComponent(WORKFLOW_ID) +
      "&session_id=" + encodeURIComponent(SESSION_ID);
    fetch(url, {
      headers: { Authorization: "Bearer " + TOKEN, Accept: "text/event-stream" },
      signal: sseAbort.signal,
    })
      .then(function (res) {
        if (!res.ok || !res.body) {
          scheduleSseReconnect();
          return;
        }
        bannerEl.style.display = "none";
        var reader = res.body.getReader();
        var decoder = new TextDecoder();
        var buffer = "";

        function pump() {
          return reader.read().then(function (r) {
            if (r.done) {
              scheduleSseReconnect();
              return;
            }
            buffer += decoder.decode(r.value, { stream: true });
            var lines = buffer.split("\n");
            buffer = lines.pop();
            for (var i = 0; i < lines.length; i++) {
              var line = lines[i].trim();
              if (line.indexOf("data:") !== 0) continue;
              var raw = line.slice(5).trim();
              if (!raw || raw === "ping") continue;
              var evt;
              try {
                evt = JSON.parse(raw);
              } catch (e) {
                continue;
              }
              handleEvent(evt);
            }
            return pump();
          });
        }
        return pump();
      })
      .catch(function () {
        scheduleSseReconnect();
      });
  }

  function handleEvent(evt) {
    if (!evt || evt.event !== "scheduler-status") return;
    var payload = evt.payload;
    if (!payload || payload.workflow_id !== WORKFLOW_ID) return;
    if (payload.status !== "waiting" && payload.status !== "error") return;
    if (!awaitingReply) return;

    clearPendingTimer();
    setAwaiting(false);

    if (payload.status === "error") {
      removeTyping();
      appendBubble("error", payload.last_error || "Workflow run failed.");
      return;
    }
    var result = payload.last_result;
    if (!result) {
      removeTyping();
      return;
    }
    if (!result.success) {
      removeTyping();
      var errLog = null;
      if (Array.isArray(result.logs)) {
        for (var i = 0; i < result.logs.length; i++) {
          if (result.logs[i] && result.logs[i].level === "error") {
            errLog = result.logs[i].message;
            break;
          }
        }
      }
      appendBubble("error", result.error || errLog || "Workflow run failed.");
      return;
    }
    var raw = outputNodeId && result.node_outputs ? result.node_outputs[outputNodeId] : undefined;
    renderReply(raw);
  }

  connectSse();
})();
