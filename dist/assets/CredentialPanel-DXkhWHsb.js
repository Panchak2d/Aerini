import{_ as e,a as t,c as n,g as r,i,l as a,n as o,o as s,r as c,s as l,u}from"./index-mJOjeF2V.js";var d=[{value:`api_key`,label:`API Key`,hint:`A plain API key passed as a header or query param`,placeholder:`sk-… or your API key`},{value:`bearer`,label:`Bearer Token`,hint:`Will be sent as Authorization: Bearer <value>`,placeholder:`eyJ… or your token`},{value:`basic`,label:`Basic Auth`,hint:`Enter as username:password`,placeholder:`username:password`},{value:`oauth`,label:`OAuth Token`,hint:`An OAuth access or refresh token`,placeholder:`ya29.… or your OAuth token`},{value:`other`,label:`Other / Custom`,hint:`Any custom secret value`,placeholder:`Your secret value`}],f=class{el;creds=[];usage={};editing=null;busy=!1;constructor(){this.el=document.getElementById(`cred-panel`),this.el||(this.el=document.createElement(`div`),this.el.id=`cred-panel`,this.el.className=`cred-panel hidden`,document.body.appendChild(this.el))}async show(){this.editing=null,this.el.classList.remove(`hidden`),await this.refresh()}hide(){this.el.classList.add(`hidden`)}async refresh(){let[e,t]=await Promise.all([a().catch(()=>[]),n().catch(()=>({}))]);this.creds=e,this.usage=t,this.render()}async backupEncryptionKey(){if(this.busy||!await r(`This reveals the master key that encrypts every credential saved here. Anyone who gets it can decrypt all of them. Store it somewhere as secure as the credentials themselves — a password manager, not a plain text file on this machine. It will only be shown this once per click.`,!0,`Show Key`))return;this.busy=!0;let e=this.el.querySelector(`#cred-key-backup`);e.disabled=!0;let n=e.textContent;e.textContent=`Loading…`;try{let e=await t();this.renderKeyBanner(e)}catch(e){await r(`Could not read the encryption key: ${e}`)}finally{e.disabled=!1,e.textContent=n,this.busy=!1}}renderKeyBanner(t){let n=this.el.querySelector(`#cred-key-banner-slot`);if(!n)return;n.innerHTML=`
      <div class="esp-run-secret-banner">
        <div class="esp-run-secret-title">⚠ Your encryption key — save it now</div>
        <div class="esp-run-secret-desc">
          If the OS keychain entry holding this is ever lost, this is the <strong>only</strong> way
          to recover every credential saved above. To restore: stop the app, place this exact value
          in the key file the app expects (see <code>docs/credentials.md</code>), then restart —
          it's picked up automatically.
        </div>
        <div class="esp-run-secret-row">
          <code class="esp-run-secret-value">${e(t)}</code>
          <button class="esp-run-secret-copy" type="button">Copy</button>
        </div>
      </div>`;let r=n.querySelector(`.esp-run-secret-copy`);r.addEventListener(`click`,()=>{navigator.clipboard.writeText(t).then(()=>{r.textContent=`Copied!`,setTimeout(()=>{r.textContent=`Copy`},2e3)}).catch(()=>{r.textContent=`Copy failed`,setTimeout(()=>{r.textContent=`Copy`},2e3)})})}render(){let e=this.creds.length===0;this.el.innerHTML=`
      <div class="cred-panel-backdrop"></div>
      <div class="cred-panel-box">
        <div class="cred-panel-header">
          <div>
            <div class="cred-panel-title">Credentials</div>
            <div class="cred-panel-subtitle">API keys and service credentials — stored encrypted on your device. Never sent anywhere.</div>
          </div>
          <button class="cred-panel-close" id="cred-close" aria-label="Close">
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round">
              <line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/>
            </svg>
          </button>
        </div>

        <div class="cred-panel-body" id="cred-panel-body">
          <div class="cred-key-backup-row">
            <button class="cred-key-backup-btn" id="cred-key-backup" type="button">Backup Encryption Key</button>
            <span class="cred-key-backup-hint">Recover access if the OS keychain entry is ever lost — read this before you need it.</span>
          </div>
          <div id="cred-key-banner-slot"></div>
          ${e?`
          <div class="cred-empty-state">
            <svg width="32" height="32" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" opacity="0.4"><path d="M10 13a5 5 0 0 0 7.54.54l3-3a5 5 0 0 0-7.07-7.07l-1.72 1.71"/><path d="M14 11a5 5 0 0 0-7.54-.54l-3 3a5 5 0 0 0 7.07 7.07l1.71-1.71"/></svg>
            <div class="cred-empty-state-title">No credentials yet</div>
            <div class="cred-empty-state-desc">Add your first API key or service credential below. Credentials are used by nodes like HTTP Request, AI Prompt, and Send Email.</div>
          </div>`:`
          <div class="cred-list" id="cred-list">
            ${this.renderList()}
          </div>`}

          <div class="cred-add-section${this.editing?` is-editing`:``}">
            <div class="cred-add-header">
              <div class="cred-add-title">${this.formTitle()}</div>
            </div>
            <div class="cred-form" id="cred-form">
              ${this.renderForm()}
            </div>
          </div>
        </div>

        <div class="cred-panel-footer">
          <div class="cred-save-error hidden" id="cred-save-error"></div>
          <div class="cred-footer-actions">
            ${this.editing?`<button type="button" class="cred-cancel-btn" id="cred-cancel">Cancel</button>`:``}
            <button class="btn-primary cred-save-btn" id="cred-save">${this.editing?`Save Changes`:`Save Credential`}</button>
          </div>
        </div>
      </div>`,this.el.querySelector(`#cred-close`).addEventListener(`click`,()=>this.hide()),this.el.querySelector(`.cred-panel-backdrop`).addEventListener(`click`,()=>this.hide()),this.el.querySelector(`#cred-key-backup`).addEventListener(`click`,()=>this.backupEncryptionKey()),this.bindForm(),this.bindList()}formTitle(){return this.editing?`Editing — ${e(this.editing.name)}`:this.creds.length===0?`Add your first credential`:`Add a credential`}renderList(){return this.creds.length?this.creds.map(t=>{let n=this.usage[t.id]??[],r=n.length?`Used by ${n.length} workflow${n.length===1?``:`s`}`:`Not used by any workflow`,i=n.length?n.join(`, `):r;return`
      <div class="cred-item${this.editing?.id===t.id?` editing`:``}" data-id="${e(t.id)}">
        <div class="cred-item-icon"><svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><circle cx="7.5" cy="15.5" r="3.5"/><path d="M21 2l-9.6 9.6"/><path d="M15.5 7.5l3 3L22 7l-3-3"/></svg></div>
        <div class="cred-item-info">
          <div class="cred-item-name">${e(t.name)}</div>
          <div class="cred-item-id">${e(t.id)}</div>
          <div class="cred-item-usage${n.length?``:` unused`}" title="${e(i)}">${e(r)}</div>
        </div>
        <span class="cred-item-type">${e(p(t.cred_type))}</span>
        <div class="cred-item-actions">
          <button class="cred-item-edit" data-id="${e(t.id)}" title="View / edit this credential">Edit</button>
          <button class="cred-item-del" data-id="${e(t.id)}" title="Delete this credential">Delete</button>
        </div>
      </div>`}).join(``):`<div class="cred-empty">
        No credentials saved yet. Add your first API key below.
      </div>`}renderForm(){let t=this.editing,n=d.find(e=>e.value===t?.cred_type)??d[0];return`
      <div class="field-group">
        <label class="field-label">Type</label>
        <div id="cred-type-wrap"></div>
        <div class="field-hint" id="cred-type-hint">${e(n.hint)}</div>
      </div>
      <div class="field-group">
        <label class="field-label">Name</label>
        <input id="cred-name" type="text" placeholder="e.g. OpenAI Production Key" autocomplete="off" value="${e(t?.name??``)}" />
        <div class="field-hint">A label to identify this credential in the UI</div>
      </div>
      <div class="field-group">
        <label class="field-label">ID / Key</label>
        <input id="cred-id" type="text" placeholder="e.g. openai_prod (no spaces)" autocomplete="off" value="${e(t?.id??``)}" ${t?`readonly`:``} />
        <div class="field-hint">${t?`ID can't be changed after creation — delete and re-add to use a different one`:`Short identifier used in nodes — auto-filled from name`}</div>
      </div>
      <div class="field-group">
        <label class="field-label">Secret Value</label>
        <div class="cred-secret-wrap">
          <input id="cred-value" type="password" placeholder="${e(n.placeholder)}" autocomplete="new-password" value="${e(t?.value??``)}" />
          <button type="button" class="cred-show-btn" id="cred-show" title="Show / hide" aria-label="Show or hide secret value">
            <svg id="cred-eye-icon" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round">
              <path d="M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z"/><circle cx="12" cy="12" r="3"/>
            </svg>
          </button>
        </div>
        <div class="field-hint">Stored encrypted on your device. Never sent to Aerini servers.</div>
      </div>
      <details class="cred-advanced"${t&&(t.provider||t.model||t.base_url)?` open`:``}>
        <summary class="cred-advanced-summary">Advanced (optional) — provider, model, base URL</summary>
        <div class="field-group">
          <label class="field-label">Provider</label>
          <input id="cred-meta-provider" type="text" placeholder="e.g. openai, anthropic, gemini" autocomplete="off" value="${e(t?.provider??``)}" />
        </div>
        <div class="field-group">
          <label class="field-label">Model</label>
          <div class="field-multiline-wrap">
            <input id="cred-meta-model" type="text" placeholder="e.g. gpt-4o, claude-sonnet-4-6" autocomplete="off" value="${e(t?.model??``)}" />
            <button type="button" class="code-load-btn" id="cred-fetch-models">Fetch Models</button>
            <div id="cred-model-list-slot"></div>
          </div>
        </div>
        <div class="field-group">
          <label class="field-label">Base URL</label>
          <input id="cred-meta-base-url" type="text" placeholder="Leave blank for provider default" autocomplete="off" value="${e(t?.base_url??``)}" />
        </div>
        <div class="field-hint">Not secret — used to auto-fill matching fields on AI nodes when this credential is selected.</div>
      </details>`}bindForm(){let e=this.el.querySelector(`#cred-save`),t=this.el.querySelector(`#cred-cancel`),n=this.el.querySelector(`#cred-save-error`),r=this.el.querySelector(`#cred-name`),i=this.el.querySelector(`#cred-id`),a=this.el.querySelector(`#cred-value`),s=this.el.querySelector(`#cred-type-hint`),l=this.el.querySelector(`#cred-type-wrap`),f=this.el.querySelector(`#cred-show`),p=this.el.querySelector(`#cred-eye-icon`),m=this.el.querySelector(`#cred-meta-provider`),g=this.el.querySelector(`#cred-meta-model`),_=this.el.querySelector(`#cred-meta-base-url`),v=this.el.querySelector(`#cred-fetch-models`),y=this.el.querySelector(`#cred-model-list-slot`),b=this.editing?.cred_type??d[0].value;l.appendChild(h(d,e=>{s.textContent=e.hint,a.placeholder=e.placeholder,b=e.value},b));let x=!!this.editing;i.addEventListener(`input`,()=>{x=!0}),r.addEventListener(`input`,()=>{x||(i.value=r.value.trim().toLowerCase().replace(/[^a-z0-9]+/g,`_`).replace(/^_+|_+$/g,``).slice(0,40))}),f.addEventListener(`click`,()=>{let e=a.type===`text`;a.type=e?`password`:`text`,p.innerHTML=e?`<path d="M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z"/><circle cx="12" cy="12" r="3"/>`:`<path d="M17.94 17.94A10.07 10.07 0 0 1 12 20c-7 0-11-8-11-8a18.45 18.45 0 0 1 5.06-5.94"/><path d="M9.9 4.24A9.12 9.12 0 0 1 12 4c7 0 11 8 11 8a18.5 18.5 0 0 1-2.16 3.19"/><line x1="1" y1="1" x2="23" y2="23"/>`});let S=`Fetch Models`;v.addEventListener(`click`,async()=>{v.disabled=!0,v.textContent=`Fetching…`;try{let e=m.value.trim()||`auto`,t=_.value.trim(),n=await c(e,t,a.value);if(!n.length)throw Error(`no models returned`);y.innerHTML=``;let r=document.createElement(`div`);r.className=`config-hint`,r.textContent=`${n.length} model${n.length===1?``:`s`} found — select one, or keep typing above.`,y.appendChild(r),y.appendChild(o(n,g.value,e=>{g.value=e})),v.textContent=S}catch{v.textContent=`Couldn't fetch — try again`,setTimeout(()=>{v.textContent=S},2500)}finally{v.disabled=!1}}),t?.addEventListener(`click`,()=>this.cancelEdit()),e.addEventListener(`click`,async()=>{let t=r.value.trim(),o=i.value.trim().replace(/\s+/g,`_`),s=a.value;if(n.classList.add(`hidden`),!t){this.showFormError(`Name is required`);return}if(!o){this.showFormError(`ID is required`);return}if(!s&&m.value.trim()!==`local`){this.showFormError(`Secret value is required`);return}if(!/^[a-zA-Z0-9_-]+$/.test(o)){this.showFormError(`ID can only contain letters, numbers, underscores, and hyphens`);return}e.disabled=!0,e.textContent=`Saving…`;try{let e=m.value.trim(),n=g.value.trim(),r=_.value.trim();await u({id:o,name:t,value:s,cred_type:b,provider:e||void 0,model:n||void 0,base_url:r||void 0}),this.editing=null,await this.refresh()}catch(t){this.showFormError(`Save failed: ${t}`),e.disabled=!1,e.textContent=this.editing?`Save Changes`:`Save Credential`}})}showFormError(e){let t=this.el.querySelector(`#cred-save-error`);t.textContent=e,t.classList.remove(`hidden`)}bindList(){this.el.querySelectorAll(`.cred-item-edit`).forEach(e=>{e.addEventListener(`click`,()=>this.startEdit(e.dataset.id,e))}),this.el.querySelectorAll(`.cred-item-del`).forEach(e=>{e.addEventListener(`click`,async()=>{if(!this.busy){this.busy=!0;try{let t=e.dataset.id,n=this.creds.find(e=>e.id===t);if(!await r(`Delete credential "${n?.name??t}"? Nodes using it will stop working.`,!0,`Delete`))return;e.disabled=!0,e.textContent=`Deleting…`;try{await i(t),this.editing?.id===t&&(this.editing=null),await this.refresh()}catch(t){e.disabled=!1,e.textContent=`Delete`,await r(`Delete failed: ${t}`)}}finally{this.busy=!1}}})})}async startEdit(e,t){if(this.busy)return;let n=this.creds.find(t=>t.id===e);if(!n)return;this.busy=!0,t.disabled=!0;let i=t.textContent;t.textContent=`Loading…`;try{let[a,o]=await Promise.all([l(e),s(e).catch(()=>null)]);if(a==null){t.disabled=!1,t.textContent=i,await r(`Could not load credential "${n.name}" — it may have been deleted.`),await this.refresh();return}this.editing={id:n.id,name:n.name,cred_type:n.cred_type,value:a,provider:o?.provider??``,model:o?.model??``,base_url:o?.base_url??``},this.render()}catch(e){t.disabled=!1,t.textContent=i,await r(`Failed to load credential: ${e}`)}finally{this.busy=!1}}cancelEdit(){this.editing=null,this.render()}};function p(e){return{api_key:`API Key`,bearer:`Bearer`,basic:`Basic Auth`,oauth:`OAuth`,other:`Custom`}[e]??e}var m=null;function h(e,t,n){m?.(),m=null;let r=e.find(e=>e.value===n)??e[0],i=document.createElement(`div`);i.className=`csel-wrap`;let a=document.createElement(`button`);a.type=`button`,a.className=`csel-trigger`;let o=document.createElement(`span`);o.className=`csel-label`,o.textContent=r.label;let s=document.createElement(`span`);s.className=`csel-arrow`,s.innerHTML=`<svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><polyline points="6 9 12 15 18 9"/></svg>`,a.appendChild(o),a.appendChild(s),i.appendChild(a);let c=document.createElement(`div`);c.className=`csel-dropdown hidden`,e.forEach(e=>{let n=document.createElement(`button`);n.type=`button`,n.className=`csel-option`+(e.value===r.value?` selected`:``),n.textContent=e.label,n.addEventListener(`mousedown`,i=>{i.preventDefault(),r=e,o.textContent=e.label,c.querySelectorAll(`.csel-option`).forEach(e=>e.classList.remove(`selected`)),n.classList.add(`selected`),c.classList.add(`hidden`),a.setAttribute(`aria-expanded`,`false`),t(e)}),c.appendChild(n)}),i.appendChild(c),a.addEventListener(`click`,()=>{let e=!c.classList.contains(`hidden`);c.classList.toggle(`hidden`,e),a.setAttribute(`aria-expanded`,String(!e))});let l=e=>{i.contains(e.target)||(c.classList.add(`hidden`),a.setAttribute(`aria-expanded`,`false`))};return document.addEventListener(`mousedown`,l),m=()=>document.removeEventListener(`mousedown`,l),i}export{f as CredentialPanel};