import{O as e,y as t}from"./index-z418KVoC.js";var n=`<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg>`;function r(t,r){let o=document.getElementById(`export-server-panel-overlay`);o&&o.remove();let s=document.getElementById(`output-drawer`);s&&!s.classList.contains(`hidden`)&&s.classList.add(`hidden`);let c=document.createElement(`div`);c.id=`export-server-panel-overlay`,c.className=`panel-overlay`;let l=document.createElement(`div`);l.className=`panel-drawer export-server-panel`,l.setAttribute(`role`,`dialog`),l.setAttribute(`aria-modal`,`true`),l.setAttribute(`aria-label`,`Export for Server`),c.appendChild(l),document.body.appendChild(c);let u=()=>{document.removeEventListener(`keydown`,d,!0),c.remove()},d=e=>{e.key===`Escape`&&u()};if(document.addEventListener(`keydown`,d,!0),c.addEventListener(`click`,e=>{e.target===c&&u()}),!t){l.innerHTML=`
      <div class="panel-header">
        <h2 class="panel-title">Export for Server</h2>
        <button class="panel-close-btn" aria-label="Close">${n}</button>
      </div>
      <div class="panel-body esp-body">
        <div class="esp-callout-warn">
          No workflow is open. Open a workflow on the canvas first, then export it.
        </div>
      </div>`,l.querySelector(`.panel-close-btn`)?.addEventListener(`click`,u);return}l.innerHTML=`
    <div class="panel-header">
      <h2 class="panel-title">Export for Server</h2>
      <button class="panel-close-btn" aria-label="Close">${n}</button>
    </div>
    <div class="panel-body esp-body">
      <div id="esp-loading" class="esp-loading">Checking workflow…</div>
      <div id="esp-content" hidden></div>
    </div>`,l.querySelector(`.panel-close-btn`)?.addEventListener(`click`,u),Promise.all([e(`validate_workflow_for_export`,{workflowId:t}),e(`list_credentials`).catch(()=>[])]).then(([e,n])=>{let i=new Set(n.map(e=>e.id));a(l,t,e,i,r)}).catch(e=>i(l,String(e)))}function i(e,n){let r=e.querySelector(`#esp-loading`),i=e.querySelector(`#esp-content`);r&&(r.hidden=!0),i&&(i.hidden=!1,i.innerHTML=`<div class="esp-callout-error">${t(n)}</div>`)}function a(n,r,i,a,s){let c=n.querySelector(`#esp-loading`),l=n.querySelector(`#esp-content`);if(c&&(c.hidden=!0),!l)return;l.hidden=!1;let u=i.credentials.some(e=>!a.has(e.credential_id)),d=i.credentials.length===0?`<p class="esp-note">This workflow uses no credentials.</p>`:`${u?`<p class="esp-cred-warning">⚠ Fill the credentials marked below before deploying.</p>`:``}
      <table class="esp-cred-table">
        <thead><tr><th>Status</th><th>Credential</th><th>Environment variable</th><th>Used by</th></tr></thead>
        <tbody>
          ${i.credentials.map(e=>{let n=a.has(e.credential_id);return`<tr class="${n?``:`esp-cred-row--empty`}">
              <td class="esp-cred-status">${n?`<span class="esp-cred-ok"  title="Credential is filled" data-tooltip="Credential is filled">✓</span>`:`<span class="esp-cred-warn" title="Credential is empty" data-tooltip="Credential is empty">⚠</span>`}</td>
              <td>${t(e.credential_id)}</td>
              <td><code>${t(e.env_var_name)}</code></td>
              <td class="esp-muted">${t(e.node_name)}</td>
            </tr>`}).join(``)}
        </tbody>
      </table>
      <p class="esp-note">
        You will set these as environment variables on your server.
        They are never stored in the package — only their names are listed.
      </p>`,f=i.variables.length===0?``:`
    <table class="esp-cred-table">
      <thead><tr><th>Variable</th><th>Suggested env key</th></tr></thead>
      <tbody>
        ${i.variables.map(e=>`
          <tr>
            <td><code>$vars.${t(e)}</code></td>
            <td><code>AERINI_VAR_${t(e.toUpperCase())}</code></td>
          </tr>
        `).join(``)}
      </tbody>
    </table>
    <p class="esp-note">
      These are referenced via <code>{{$vars.x}}</code> in your workflow.
      Set them as environment variables on your server.
    </p>`;l.innerHTML=`
    <p class="esp-intro">
      Generates a self-contained deployment package for your workflow.
      Choose your target environment below.
    </p>

    <div class="esp-field-row">
      <div class="esp-field">
        <span class="esp-field-label">Trigger</span>
        <span class="esp-field-value">${t(i.trigger_desc)}</span>
      </div>
    </div>

    ${i.variables.length>0?`
    <h3 class="esp-section-title">Required variables</h3>
    ${f}`:``}

    <div class="esp-tab-bar" role="tablist" aria-label="Deployment target">
      <button id="esp-tab-linux" class="esp-tab esp-tab--active" role="tab"
              aria-selected="true"  aria-controls="esp-pane-linux">
        🖥 Linux Server
      </button>
      <button id="esp-tab-docker" class="esp-tab" role="tab"
              aria-selected="false" aria-controls="esp-pane-docker">
        🐳 Docker
      </button>
    </div>

    <div id="esp-pane-linux" role="tabpanel" aria-labelledby="esp-tab-linux">
      <p class="esp-note esp-pane-desc">
        Generates a zip with a self-installing systemd service.
        Unzip it on any Linux server and run <code>./install.sh</code>.
        No Rust required on the server.
      </p>

      <h3 class="esp-section-title">Credentials</h3>
      ${d}

      <h3 class="esp-section-title">Status page port</h3>
      <p class="esp-note">
        The server will serve a live status page on this port.
        Make sure it's open in your server's firewall.
      </p>
      <div class="esp-port-action-row">
        <input id="esp-port-linux" class="esp-input esp-port-input" type="number"
               min="1024" max="65535" value="7700" />
        <div class="esp-generate-wrap">
          <button id="esp-generate-linux-btn" class="esp-primary-btn">
            ⬇ Generate Linux Package
          </button>
          <span id="esp-generating-linux" hidden class="esp-generating-label">
            <span class="esp-spinner"></span>Building package…
          </span>
        </div>
      </div>

      <div id="esp-success-linux" hidden class="esp-success-block">
        <div class="esp-callout esp-mt">
          <strong>Package saved.</strong><br>
          Upload it to your server and run:<br>
          <code>unzip aerini-server-*.zip &amp;&amp; chmod +x install.sh &amp;&amp; ./install.sh</code>
        </div>
      </div>
    </div>

    <div id="esp-pane-docker" role="tabpanel" aria-labelledby="esp-tab-docker" hidden>
      <p class="esp-note esp-pane-desc">
        Generates a zip with a <code>Dockerfile</code> and <code>docker-compose.yml</code>
        for single-workflow serve mode.
        Run <code>docker compose up --build -d</code> — no Rust required on the host.
      </p>

      <h3 class="esp-section-title">Credentials</h3>
      ${d}

      <h3 class="esp-section-title">Status page port (host)</h3>
      <p class="esp-note">
        The status page will be mapped to this port on the host machine.
        The container always runs on port 7700 internally.
      </p>
      <div class="esp-port-action-row">
        <input id="esp-port-docker" class="esp-input esp-port-input" type="number"
               min="1024" max="65535" value="7700" />
        <div class="esp-generate-wrap">
          <button id="esp-generate-docker-btn" class="esp-primary-btn">
            ⬇ Generate Docker Package
          </button>
          <span id="esp-generating-docker" hidden class="esp-generating-label">
            <span class="esp-spinner"></span>Building package…
          </span>
        </div>
      </div>

      <div id="esp-success-docker" hidden class="esp-success-block">
        <div class="esp-callout esp-mt">
          <strong>Package saved.</strong><br>
          Extract the zip, fill in <code>.env</code>, then run:<br>
          <code>docker compose up --build -d</code>
        </div>
      </div>
    </div>`;let p=l.querySelector(`#esp-tab-linux`),m=l.querySelector(`#esp-tab-docker`),h=l.querySelector(`#esp-pane-linux`),g=l.querySelector(`#esp-pane-docker`),_=e=>{let t=e===`linux`;p?.classList.toggle(`esp-tab--active`,t),m?.classList.toggle(`esp-tab--active`,!t),p?.setAttribute(`aria-selected`,String(t)),m?.setAttribute(`aria-selected`,String(!t)),h&&(h.hidden=!t),g&&(g.hidden=t)};p?.addEventListener(`click`,()=>_(`linux`)),m?.addEventListener(`click`,()=>_(`docker`));let v=l.querySelector(`#esp-generate-linux-btn`),y=l.querySelector(`#esp-generating-linux`),b=l.querySelector(`#esp-success-linux`),x=l.querySelector(`#esp-port-linux`);v?.addEventListener(`click`,async()=>{let t=parseInt(x?.value??`7700`,10);if(isNaN(t)||t<1024||t>65535){s(`Port must be between 1024 and 65535`,`error`);return}v.disabled=!0,y&&(y.hidden=!1);try{let n=await e(`generate_server_package`,{request:{workflow_id:r,status_port:t}}),i=`aerini-server-${n.workflow_name.replace(/[^a-z0-9-]/gi,`_`)}.zip`,a=await e(`save_export_zip`,{zipPath:n.zip_path,filename:i}).catch(e=>{if(e!==`cancelled`)throw Error(e);return null});a&&b&&(b.hidden=!1),a&&(s(`Linux server package saved`,`success`),o(l,n.run_secret_plaintext))}catch(e){s(`Export failed: ${String(e)}`,`error`)}finally{v.disabled=!1,y&&(y.hidden=!0)}});let S=l.querySelector(`#esp-generate-docker-btn`),C=l.querySelector(`#esp-generating-docker`),w=l.querySelector(`#esp-success-docker`),T=l.querySelector(`#esp-port-docker`);S?.addEventListener(`click`,async()=>{let t=parseInt(T?.value??`7700`,10);if(isNaN(t)||t<1024||t>65535){s(`Port must be between 1024 and 65535`,`error`);return}S.disabled=!0,C&&(C.hidden=!1);try{let n=await e(`generate_docker_package`,{request:{workflow_id:r,status_port:t}}),i=`aerini-docker-${n.workflow_name.replace(/[^a-z0-9-]/gi,`_`)}.zip`,a=await e(`save_export_zip`,{zipPath:n.zip_path,filename:i}).catch(e=>{if(e!==`cancelled`)throw Error(e);return null});a&&w&&(w.hidden=!1),a&&(s(`Docker package saved`,`success`),o(l,n.run_secret_plaintext))}catch(e){s(`Export failed: ${String(e)}`,`error`)}finally{S.disabled=!1,C&&(C.hidden=!0)}})}function o(e,t){if(!e)return;e.querySelector(`.esp-run-secret-banner`)?.remove();let n=document.createElement(`div`);n.className=`esp-run-secret-banner`,n.innerHTML=`
    <div class="esp-run-secret-title">⚠ Save your run secret — shown once</div>
    <div class="esp-run-secret-desc">
      This secret authenticates <code>POST /api/run</code> and <code>GET /api/logs</code>.
      It is <strong>not stored in the zip</strong> — only its hash is. Copy it now.
    </div>
    <div class="esp-run-secret-row">
      <code class="esp-run-secret-value">${t}</code>
      <button class="esp-run-secret-copy" type="button">Copy</button>
    </div>`;let r=n.querySelector(`.esp-run-secret-copy`);r.addEventListener(`click`,()=>{navigator.clipboard.writeText(t).then(()=>{r.textContent=`Copied!`,setTimeout(()=>{r.textContent=`Copy`},2e3)}).catch(()=>{r.textContent=`Copy failed`,setTimeout(()=>{r.textContent=`Copy`},2e3)})}),e.appendChild(n)}export{r as showExportServerPanel};