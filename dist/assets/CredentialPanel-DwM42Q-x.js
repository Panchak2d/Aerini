import{a as e,b as t,c as n,d as r,f as i,i as a,l as o,o as s,p as c,s as l,u,x as d}from"./index-C5LSfqY7.js";var f=[{value:`api_key`,label:`API Key`,hint:`A plain API key passed as a header or query param`,placeholder:`sk-… or your API key`},{value:`bearer`,label:`Bearer Token`,hint:`Will be sent as Authorization: Bearer <value>`,placeholder:`eyJ… or your token`},{value:`basic`,label:`Basic Auth`,hint:`Enter as username:password`,placeholder:`username:password`},{value:`oauth`,label:`OAuth Token`,hint:`An OAuth access or refresh token`,placeholder:`ya29.… or your OAuth token`},{value:`other`,label:`Other / Custom`,hint:`Any custom secret value`,placeholder:`Your secret value`}],p=class{el;creds=[];usage={};editing=null;busy=!1;constructor(){this.el=document.getElementById(`cred-panel`),this.el||(this.el=document.createElement(`div`),this.el.id=`cred-panel`,this.el.className=`cred-panel hidden`,document.body.appendChild(this.el))}async show(){this.editing=null,this.el.classList.remove(`hidden`),await this.refresh()}hide(){this.el.classList.add(`hidden`)}async refresh(){let[e,t]=await Promise.all([i().catch(()=>[]),r().catch(()=>({}))]);this.creds=e,this.usage=t,this.render()}async backupEncryptionKey(){if(this.busy||!await t(`This reveals the master key that encrypts every credential saved here. Anyone who gets it can decrypt all of them. Store it somewhere as secure as the credentials themselves — a password manager, not a plain text file on this machine. It will only be shown this once per click.`,!0,`Show Key`))return;this.busy=!0;let e=this.el.querySelector(`#cred-key-backup`);e.disabled=!0;let r=e.textContent;e.textContent=`Loading…`;try{let e=await n();this.renderKeyBanner(e)}catch(e){await t(`Could not read the encryption key: ${e}`)}finally{e.disabled=!1,e.textContent=r,this.busy=!1}}renderKeyBanner(e){let t=this.el.querySelector(`#cred-key-banner-slot`);if(!t)return;t.innerHTML=`
      <div class="esp-run-secret-banner">
        <div class="esp-run-secret-title">⚠ Your encryption key — save it now</div>
        <div class="esp-run-secret-desc">
          If the OS keychain entry holding this is ever lost, this is the <strong>only</strong> way
          to recover every credential saved above. To restore: stop the app, place this exact value
          in the key file the app expects (see <code>docs/credentials.md</code>), then restart —
          it's picked up automatically.
        </div>
        <div class="esp-run-secret-row">
          <code class="esp-run-secret-value">${d(e)}</code>
          <button class="esp-run-secret-copy" type="button">Copy</button>
        </div>
      </div>`;let n=t.querySelector(`.esp-run-secret-copy`);n.addEventListener(`click`,()=>{navigator.clipboard.writeText(e).then(()=>{n.textContent=`Copied!`,setTimeout(()=>{n.textContent=`Copy`},2e3)}).catch(()=>{n.textContent=`Copy failed`,setTimeout(()=>{n.textContent=`Copy`},2e3)})})}render(){let e=this.creds.length===0;this.el.innerHTML=`
      <div class="cred-panel-backdrop"></div>
      <div class="cred-panel-box">
        <div class="cred-panel-header">
          <div>
            <div class="cred-panel-title">Credentials</div>
            <div class="cred-panel-subtitle">API keys and service credentials — stored encrypted on your device. Never sent anywhere.</div>
          </div>
          <button class="cred-panel-close" id="cred-close" aria-label="Close">
            <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round">
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
      </div>`,this.el.querySelector(`#cred-close`).addEventListener(`click`,()=>this.hide()),this.el.querySelector(`.cred-panel-backdrop`).addEventListener(`click`,()=>this.hide()),this.el.querySelector(`#cred-key-backup`).addEventListener(`click`,()=>this.backupEncryptionKey()),this.bindForm(),this.bindList()}formTitle(){return this.editing?`Editing — ${d(this.editing.name)}`:this.creds.length===0?`Add your first credential`:`Add a credential`}renderList(){return this.creds.length?this.creds.map(e=>{let t=this.usage[e.id]??[],n=t.length?`Used by ${t.length} workflow${t.length===1?``:`s`}`:`Not used by any workflow`,r=t.length?t.join(`, `):n;return`
      <div class="cred-item${this.editing?.id===e.id?` editing`:``}" data-id="${d(e.id)}">
        <div class="cred-item-icon"><svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><circle cx="7.5" cy="15.5" r="3.5"/><path d="M21 2l-9.6 9.6"/><path d="M15.5 7.5l3 3L22 7l-3-3"/></svg></div>
        <div class="cred-item-info">
          <div class="cred-item-name" title="${d(e.name)}">${d(e.name)}</div>
          <div class="cred-item-id" title="${d(e.id)}">${d(e.id)}</div>
          <div class="cred-item-usage${t.length?``:` unused`}" title="${d(r)}" data-tooltip="${d(r)}">${d(n)}</div>
        </div>
        <span class="cred-item-type">${d(m(e.cred_type))}</span>
        <div class="cred-item-actions">
          <button class="cred-item-edit" data-id="${d(e.id)}" title="View / edit this credential">Edit</button>
          <button class="cred-item-del" data-id="${d(e.id)}" title="Delete this credential">Delete</button>
        </div>
      </div>`}).join(``):`<div class="cred-empty">
        No credentials saved yet. Add your first API key below.
      </div>`}renderForm(){let e=this.editing,t=f.find(t=>t.value===e?.cred_type)??f[0];return`
      <div class="field-group">
        <label class="field-label">Type</label>
        <div id="cred-type-wrap"></div>
        <div class="field-hint" id="cred-type-hint">${d(t.hint)}</div>
      </div>
      <div class="field-group">
        <label class="field-label">Name</label>
        <input id="cred-name" type="text" placeholder="e.g. OpenAI Production Key" autocomplete="off" value="${d(e?.name??``)}" />
        <div class="field-hint">A label to identify this credential in the UI</div>
      </div>
      <div class="field-group">
        <label class="field-label">ID / Key</label>
        <input id="cred-id" type="text" placeholder="e.g. openai_prod (no spaces)" autocomplete="off" value="${d(e?.id??``)}" ${e?`readonly`:``} />
        <div class="field-hint">${e?`ID can't be changed after creation — delete and re-add to use a different one`:`Short identifier used in nodes — auto-filled from name`}</div>
      </div>
      <div class="field-group">
        <label class="field-label">Secret Value</label>
        <div class="cred-secret-wrap">
          <input id="cred-value" type="password" placeholder="${d(t.placeholder)}" autocomplete="new-password" value="${d(e?.value??``)}" />
          <button type="button" class="cred-show-btn" id="cred-show" title="Show / hide" data-tooltip="Show / hide" aria-label="Show or hide secret value">
            <svg id="cred-eye-icon" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round">
              <path d="M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z"/><circle cx="12" cy="12" r="3"/>
            </svg>
          </button>
        </div>
        <div class="field-hint">Stored encrypted on your device. Never sent to Aerini servers.</div>
      </div>
      <details class="cred-advanced"${e&&(e.provider||e.model||e.base_url)?` open`:``}>
        <summary class="cred-advanced-summary">Advanced (optional) — provider, model, base URL</summary>
        <div class="field-group">
          <label class="field-label">Provider</label>
          <div class="field-multiline-wrap">
            <input id="cred-meta-provider" type="text" placeholder="e.g. openai, anthropic, gemini" autocomplete="off" value="${d(e?.provider??``)}" />
            <div id="cred-provider-picker-slot"></div>
          </div>
          <div class="field-hint">Pick a known provider, or type any other value (a proxy or self-hosted alias) — the field always stays editable.</div>
        </div>
        <div class="field-group">
          <label class="field-label">Model</label>
          <div class="field-multiline-wrap">
            <input id="cred-meta-model" type="text" placeholder="e.g. gpt-4o, claude-sonnet-4-6" autocomplete="off" value="${d(e?.model??``)}" />
            <button type="button" class="code-load-btn" id="cred-fetch-models">Fetch Models</button>
            <div id="cred-model-list-slot"></div>
          </div>
        </div>
        <div class="field-group">
          <label class="field-label">Base URL</label>
          <div class="field-multiline-wrap">
            <input id="cred-meta-base-url" type="text" placeholder="Leave blank for provider default" autocomplete="off" value="${d(e?.base_url??``)}" />
            <div id="cred-base-url-help-slot"></div>
          </div>
        </div>
        <div class="field-hint">Not secret — used to auto-fill matching fields on AI nodes when this credential is selected.</div>
      </details>`}bindForm(){let t=this.el.querySelector(`#cred-save`),n=this.el.querySelector(`#cred-cancel`),r=this.el.querySelector(`#cred-save-error`),i=this.el.querySelector(`#cred-name`),o=this.el.querySelector(`#cred-id`),l=this.el.querySelector(`#cred-value`),u=this.el.querySelector(`#cred-type-hint`),d=this.el.querySelector(`#cred-type-wrap`),p=this.el.querySelector(`#cred-show`),m=this.el.querySelector(`#cred-eye-icon`),h=this.el.querySelector(`#cred-meta-provider`),_=this.el.querySelector(`#cred-meta-model`),v=this.el.querySelector(`#cred-meta-base-url`),y=this.el.querySelector(`#cred-fetch-models`),b=this.el.querySelector(`#cred-model-list-slot`),x=this.el.querySelector(`#cred-provider-picker-slot`),S=this.editing?.cred_type??f[0].value;d.appendChild(g(f,e=>{u.textContent=e.hint,l.placeholder=e.placeholder,S=e.value},S));let C=[`openai`,`anthropic`,`gemini`,`local`];function w(){x.innerHTML=``,x.appendChild(a(C,h.value.trim(),e=>{h.value=e,w(),D()}))}w();let T=`http://localhost:11434/v1`,E=this.el.querySelector(`#cred-base-url-help-slot`);function D(){let e=h.value.trim()===`local`;if(v.placeholder=e?`e.g. ${T} (Ollama)`:`Leave blank for provider default`,E.innerHTML=``,!e)return;let t=document.createElement(`button`);t.type=`button`,t.className=`code-load-btn`,t.textContent=`Use Ollama defaults`,t.addEventListener(`click`,()=>{v.value=T}),E.appendChild(t)}D(),h.addEventListener(`input`,()=>{w(),D()});let O=!!this.editing;o.addEventListener(`input`,()=>{O=!0}),i.addEventListener(`input`,()=>{O||(o.value=i.value.trim().toLowerCase().replace(/[^a-z0-9]+/g,`_`).replace(/^_+|_+$/g,``).slice(0,40))}),p.addEventListener(`click`,()=>{let e=l.type===`text`;l.type=e?`password`:`text`,m.innerHTML=e?`<path d="M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z"/><circle cx="12" cy="12" r="3"/>`:`<path d="M17.94 17.94A10.07 10.07 0 0 1 12 20c-7 0-11-8-11-8a18.45 18.45 0 0 1 5.06-5.94"/><path d="M9.9 4.24A9.12 9.12 0 0 1 12 4c7 0 11 8 11 8a18.5 18.5 0 0 1-2.16 3.19"/><line x1="1" y1="1" x2="23" y2="23"/>`});let k=`Fetch Models`;y.addEventListener(`click`,async()=>{y.disabled=!0,y.textContent=`Fetching…`;try{let e=h.value.trim()||`auto`,t=v.value.trim(),n=await s(e,t,l.value);if(!n.length)throw Error(`no models returned`);b.innerHTML=``;let r=document.createElement(`div`);r.className=`config-hint`,r.textContent=`${n.length} model${n.length===1?``:`s`} found — select one, or keep typing above.`,b.appendChild(r),b.appendChild(a(n,_.value,e=>{_.value=e})),y.textContent=k}catch(t){y.textContent=`Couldn't fetch — try again`,b.innerHTML=``;let n=document.createElement(`div`);n.className=`config-hint config-hint-warn`,n.textContent=e(t),b.appendChild(n),setTimeout(()=>{y.textContent=k},2500)}finally{y.disabled=!1}}),n?.addEventListener(`click`,()=>this.cancelEdit()),t.addEventListener(`click`,async()=>{let e=i.value.trim(),n=o.value.trim().replace(/\s+/g,`_`),a=l.value;if(r.classList.add(`hidden`),!e){this.showFormError(`Name is required`);return}if(!n){this.showFormError(`ID is required`);return}if(!a&&h.value.trim()!==`local`){this.showFormError(`Secret value is required`);return}if(!/^[a-zA-Z0-9_-]+$/.test(n)){this.showFormError(`ID can only contain letters, numbers, underscores, and hyphens`);return}t.disabled=!0,t.textContent=`Saving…`;try{let t=h.value.trim(),r=_.value.trim(),i=v.value.trim();await c({id:n,name:e,value:a,cred_type:S,provider:t||void 0,model:r||void 0,base_url:i||void 0}),this.editing=null,await this.refresh()}catch(e){this.showFormError(`Save failed: ${e}`),t.disabled=!1,t.textContent=this.editing?`Save Changes`:`Save Credential`}})}showFormError(e){let t=this.el.querySelector(`#cred-save-error`);t.textContent=e,t.classList.remove(`hidden`)}bindList(){this.el.querySelectorAll(`.cred-item-edit`).forEach(e=>{e.addEventListener(`click`,()=>this.startEdit(e.dataset.id,e))}),this.el.querySelectorAll(`.cred-item-del`).forEach(e=>{e.addEventListener(`click`,async()=>{if(!this.busy){this.busy=!0;try{let n=e.dataset.id,r=this.creds.find(e=>e.id===n);if(!await t(`Delete credential "${r?.name??n}"? Nodes using it will stop working.`,!0,`Delete`))return;e.disabled=!0,e.textContent=`Deleting…`;try{await l(n),this.editing?.id===n&&(this.editing=null),await this.refresh()}catch(n){e.disabled=!1,e.textContent=`Delete`,await t(`Delete failed: ${n}`)}}finally{this.busy=!1}}})})}async startEdit(e,n){if(this.busy)return;let r=this.creds.find(t=>t.id===e);if(!r)return;this.busy=!0,n.disabled=!0;let i=n.textContent;n.textContent=`Loading…`;try{let[a,s]=await Promise.all([u(e),o(e).catch(()=>null)]);if(a==null){n.disabled=!1,n.textContent=i,await t(`Could not load credential "${r.name}" — it may have been deleted.`),await this.refresh();return}this.editing={id:r.id,name:r.name,cred_type:r.cred_type,value:a,provider:s?.provider??``,model:s?.model??``,base_url:s?.base_url??``},this.render()}catch(e){n.disabled=!1,n.textContent=i,await t(`Failed to load credential: ${e}`)}finally{this.busy=!1}}cancelEdit(){this.editing=null,this.render()}};function m(e){return{api_key:`API Key`,bearer:`Bearer`,basic:`Basic Auth`,oauth:`OAuth`,other:`Custom`}[e]??e}var h=null;function g(e,t,n){h?.(),h=null;let r=e.find(e=>e.value===n)??e[0],i=document.createElement(`div`);i.className=`csel-wrap`;let a=document.createElement(`button`);a.type=`button`,a.className=`csel-trigger`;let o=document.createElement(`span`);o.className=`csel-label`,o.textContent=r.label,o.title=r.label;let s=document.createElement(`span`);s.className=`csel-arrow`,s.innerHTML=`<svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><polyline points="6 9 12 15 18 9"/></svg>`,a.appendChild(o),a.appendChild(s),i.appendChild(a);let c=document.createElement(`div`);c.className=`csel-dropdown hidden`,e.forEach(e=>{let n=document.createElement(`button`);n.type=`button`,n.className=`csel-option`+(e.value===r.value?` selected`:``),n.textContent=e.label,n.addEventListener(`mousedown`,i=>{i.preventDefault(),r=e,o.textContent=e.label,o.title=e.label,c.querySelectorAll(`.csel-option`).forEach(e=>e.classList.remove(`selected`)),n.classList.add(`selected`),c.classList.add(`hidden`),a.setAttribute(`aria-expanded`,`false`),t(e)}),c.appendChild(n)}),i.appendChild(c),a.addEventListener(`click`,()=>{let e=!c.classList.contains(`hidden`);c.classList.toggle(`hidden`,e),a.setAttribute(`aria-expanded`,String(!e))});let l=e=>{i.contains(e.target)||(c.classList.add(`hidden`),a.setAttribute(`aria-expanded`,`false`))};return document.addEventListener(`mousedown`,l),h=()=>document.removeEventListener(`mousedown`,l),i}export{p as CredentialPanel};