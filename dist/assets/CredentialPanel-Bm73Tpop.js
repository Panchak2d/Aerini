import{C as e,S as t,a as n,c as r,d as i,f as a,i as o,l as s,m as c,o as l,p as u,s as d,u as f,x as p}from"./index-BGECBc4n.js";var m=[{value:`api_key`,label:`API Key`,hint:`A plain API key passed as a header or query param`,placeholder:`sk-… or your API key`},{value:`bearer`,label:`Bearer Token`,hint:`Will be sent as Authorization: Bearer <value>`,placeholder:`eyJ… or your token`},{value:`basic`,label:`Basic Auth`,hint:`Enter as username:password`,placeholder:`username:password`},{value:`oauth`,label:`OAuth Token`,hint:`An OAuth access or refresh token`,placeholder:`ya29.… or your OAuth token`},{value:`other`,label:`Other / Custom`,hint:`Any custom secret value`,placeholder:`Your secret value`}],h=6e4,g=`••• (unchanged)`,_=class{el;creds=[];usage={};editing=null;busy=!1;keyBannerTimer=null;clipboardTimer=null;ollamaCtl=null;constructor(){this.el=document.getElementById(`cred-panel`),this.el||(this.el=document.createElement(`div`),this.el.id=`cred-panel`,this.el.className=`cred-panel hidden`,document.body.appendChild(this.el))}async show(){this.editing=null,this.el.classList.remove(`hidden`),await this.refresh()}hide(){this.clearKeyBanner(),this.disposeOllama(),this.el.classList.add(`hidden`)}disposeOllama(){this.ollamaCtl?.dispose(),this.ollamaCtl=null}async refresh(){let[e,t]=await Promise.all([u().catch(()=>[]),a().catch(()=>({}))]);this.creds=e,this.usage=t,this.render()}async backupEncryptionKey(){if(this.busy)return;this.busy=!0;let e=this.el.querySelector(`#cred-key-backup`);e.disabled=!0;let t=e.textContent;e.textContent=`Loading…`;try{let e=await s();this.renderKeyBanner(e)}catch(e){String(e)!==`CANCELLED`&&await p(`Could not read the encryption key: ${e}`)}finally{e.disabled=!1,e.textContent=t,this.busy=!1}}clearKeyBanner(){this.keyBannerTimer&&clearTimeout(this.keyBannerTimer),this.keyBannerTimer=null;let e=this.el.querySelector(`#cred-key-banner-slot`);e&&(e.innerHTML=``)}scheduleClipboardClear(e){this.clipboardTimer&&clearTimeout(this.clipboardTimer),this.clipboardTimer=setTimeout(()=>{this.clipboardTimer=null,v(e)},h)}renderKeyBanner(e){let n=this.el.querySelector(`#cred-key-banner-slot`);if(!n)return;n.innerHTML=`
      <div class="esp-run-secret-banner">
        <div class="esp-run-secret-title">⚠ Your encryption key — save it now</div>
        <div class="esp-run-secret-desc">
          If the OS keychain entry holding this is ever lost, this is the <strong>only</strong> way
          to recover every credential saved above. To restore: stop the app, place this exact value
          in the key file the app expects (see <code>docs/credentials.md</code>), then restart —
          it's picked up automatically. This panel hides the key after 60 seconds.
        </div>
        <div class="esp-run-secret-row">
          <code class="esp-run-secret-value"></code>
          <button class="esp-run-secret-copy" type="button">Copy</button>
        </div>
      </div>`,n.querySelector(`.esp-run-secret-value`).textContent=e;let r=n.querySelector(`.esp-run-secret-copy`);r.addEventListener(`click`,()=>{t(r,e),this.scheduleClipboardClear(e)}),this.keyBannerTimer&&clearTimeout(this.keyBannerTimer),this.keyBannerTimer=setTimeout(()=>this.clearKeyBanner(),h)}render(){this.disposeOllama();let e=this.creds.length===0;this.el.innerHTML=`
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
      </div>`,this.el.querySelector(`#cred-close`).addEventListener(`click`,()=>this.hide()),this.el.querySelector(`.cred-panel-backdrop`).addEventListener(`click`,()=>this.hide()),this.el.querySelector(`#cred-key-backup`).addEventListener(`click`,()=>this.backupEncryptionKey()),this.bindForm(),this.bindList()}formTitle(){return this.editing?`Editing — ${e(this.editing.name)}`:this.creds.length===0?`Add your first credential`:`Add a credential`}renderList(){return this.creds.length?this.creds.map(t=>{let n=this.usage[t.id]??[],r=n.length?`Used by ${n.length} workflow${n.length===1?``:`s`}`:`Not used by any workflow`,i=n.length?n.join(`, `):r;return`
      <div class="cred-item${this.editing?.id===t.id?` editing`:``}" data-id="${e(t.id)}">
        <div class="cred-item-icon"><svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><circle cx="7.5" cy="15.5" r="3.5"/><path d="M21 2l-9.6 9.6"/><path d="M15.5 7.5l3 3L22 7l-3-3"/></svg></div>
        <div class="cred-item-info">
          <div class="cred-item-name" title="${e(t.name)}">${e(t.name)}</div>
          <div class="cred-item-id" title="${e(t.id)}">${e(t.id)}</div>
          <div class="cred-item-usage${n.length?``:` unused`}" title="${e(i)}" data-tooltip="${e(i)}">${e(r)}</div>
        </div>
        <span class="cred-item-type">${e(y(t.cred_type))}</span>
        <div class="cred-item-actions">
          <button class="cred-item-edit" data-id="${e(t.id)}" title="View / edit this credential">Edit</button>
          <button class="cred-item-del" data-id="${e(t.id)}" title="Delete this credential">Delete</button>
        </div>
      </div>`}).join(``):`<div class="cred-empty">
        No credentials saved yet. Add your first API key below.
      </div>`}renderForm(){let t=this.editing,n=m.find(e=>e.value===t?.cred_type)??m[0];return`
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
          <input id="cred-value" type="password" placeholder="${e(t?g:n.placeholder)}" autocomplete="new-password" value="" />
          <button type="button" class="cred-show-btn" id="cred-show" title="Show / hide" data-tooltip="Show / hide" aria-label="Show or hide secret value">
            <svg id="cred-eye-icon" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round">
              <path d="M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z"/><circle cx="12" cy="12" r="3"/>
            </svg>
          </button>
        </div>
        <div class="field-hint">${t?`Leave blank to keep the saved secret. Saved secrets are never shown.`:`Stored encrypted on your device. Never sent to Aerini servers.`}</div>
      </div>
      <details class="cred-advanced"${t&&(t.provider||t.model||t.base_url)?` open`:``}>
        <summary class="cred-advanced-summary">Advanced (optional) — provider, model, base URL</summary>
        <div class="field-group">
          <label class="field-label">Provider</label>
          <div class="field-multiline-wrap">
            <input id="cred-meta-provider" type="text" placeholder="e.g. openai, anthropic, gemini" autocomplete="off" value="${e(t?.provider??``)}" />
            <div id="cred-provider-picker-slot"></div>
          </div>
          <div class="field-hint">Pick a known provider, or type any other value (a proxy or self-hosted alias) — the field always stays editable.</div>
        </div>
        <div class="field-group">
          <label class="field-label">Model</label>
          <div class="field-multiline-wrap">
            <input id="cred-meta-model" type="text" placeholder="e.g. gpt-5.6, claude-sonnet-5-5" autocomplete="off" value="${e(t?.model??``)}" />
            <button type="button" class="code-load-btn" id="cred-fetch-models">Fetch Models</button>
            <div id="cred-ollama-slot"></div>
            <div id="cred-model-list-slot"></div>
          </div>
        </div>
        <div class="field-group">
          <label class="field-label">Base URL</label>
          <div class="field-multiline-wrap">
            <input id="cred-meta-base-url" type="text" placeholder="Leave blank for provider default" autocomplete="off" value="${e(t?.base_url??``)}" />
            <div id="cred-base-url-help-slot"></div>
          </div>
        </div>
        <div class="field-hint">Not secret — used to auto-fill matching fields on AI nodes when this credential is selected.</div>
      </details>`}bindForm(){let e=this.el.querySelector(`#cred-save`),t=this.el.querySelector(`#cred-cancel`),r=this.el.querySelector(`#cred-save-error`),a=this.el.querySelector(`#cred-name`),s=this.el.querySelector(`#cred-id`),u=this.el.querySelector(`#cred-value`),f=this.el.querySelector(`#cred-type-hint`),p=this.el.querySelector(`#cred-type-wrap`),h=this.el.querySelector(`#cred-show`),_=this.el.querySelector(`#cred-eye-icon`),v=this.el.querySelector(`#cred-meta-provider`),y=this.el.querySelector(`#cred-meta-model`),b=this.el.querySelector(`#cred-meta-base-url`),S=this.el.querySelector(`#cred-fetch-models`),C=this.el.querySelector(`#cred-model-list-slot`),w=this.el.querySelector(`#cred-ollama-slot`),T=``,E=()=>{let e=JSON.stringify(R());e!==T&&(T=e,this.ollamaCtl?.refresh())},D=this.el.querySelector(`#cred-provider-picker-slot`),O=this.editing?.cred_type??m[0].value;p.appendChild(x(m,e=>{f.textContent=e.hint,u.placeholder=this.editing?g:e.placeholder,O=e.value},O));let k=[`openai`,`anthropic`,`gemini`,`local`];function A(){D.innerHTML=``,D.appendChild(n(k,v.value.trim(),e=>{v.value=e,A(),N(),E()}))}A();let j=`http://localhost:11434/v1`,M=this.el.querySelector(`#cred-base-url-help-slot`);function N(){let e=v.value.trim()===`local`;if(b.placeholder=e?`e.g. ${j} (Ollama)`:`Leave blank for provider default`,M.innerHTML=``,!e)return;let t=document.createElement(`button`);t.type=`button`,t.className=`code-load-btn`,t.textContent=`Use Ollama defaults`,t.addEventListener(`click`,()=>{b.value=j,E()}),M.appendChild(t)}N(),v.addEventListener(`input`,()=>{A(),N(),E()}),b.addEventListener(`input`,E),u.addEventListener(`input`,E);let P=!!this.editing;s.addEventListener(`input`,()=>{P=!0}),a.addEventListener(`input`,()=>{P||(s.value=a.value.trim().toLowerCase().replace(/[^a-z0-9]+/g,`_`).replace(/^_+|_+$/g,``).slice(0,40))}),h.addEventListener(`click`,()=>{let e=u.type===`text`;u.type=e?`password`:`text`,_.innerHTML=e?`<path d="M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z"/><circle cx="12" cy="12" r="3"/>`:`<path d="M17.94 17.94A10.07 10.07 0 0 1 12 20c-7 0-11-8-11-8a18.45 18.45 0 0 1 5.06-5.94"/><path d="M9.9 4.24A9.12 9.12 0 0 1 12 4c7 0 11 8 11 8a18.5 18.5 0 0 1-2.16 3.19"/><line x1="1" y1="1" x2="23" y2="23"/>`});let F=`Fetch Models`,I=0,L=async(e=!1)=>{let t=++I;S.disabled=!0,S.textContent=`Fetching…`;try{let r=v.value.trim()||`auto`,a=b.value.trim(),o=this.editing?.id,s=o&&!u.value?await i(o,r,a):await d(r,a,u.value);if(t!==I)return;if(!s.length&&e){C.innerHTML=``;let e=document.createElement(`div`);e.className=`config-hint`,e.textContent=`No models are installed on this server.`,C.appendChild(e),S.textContent=F;return}if(!s.length)throw Error(`no models returned`);C.innerHTML=``;let c=document.createElement(`div`);c.className=`config-hint`,c.textContent=`${s.length} model${s.length===1?``:`s`} found — select one, or keep typing above.`,C.appendChild(c),C.appendChild(n(s,y.value,e=>{y.value=e})),S.textContent=F}catch(e){if(t!==I)return;S.textContent=`Couldn't fetch — try again`,C.innerHTML=``;let n=document.createElement(`div`);n.className=`config-hint config-hint-warn`,n.textContent=l(e),C.appendChild(n),setTimeout(()=>{S.textContent=F},2500)}finally{t===I&&(S.disabled=!1)}};S.addEventListener(`click`,()=>{L()});let R=()=>{let e=v.value.trim()||`auto`,t=b.value.trim();if(e!==`local`&&e!==`auto`||!t)return null;if(u.value)return{provider:e,base_url:t,api_key:u.value};let n=this.editing?.id;return n?{provider:e,base_url:t,credential_id:n}:{provider:e,base_url:t}};T=JSON.stringify(R()),this.ollamaCtl=o({getTarget:R,getModel:()=>y.value,onModelsChanged:()=>{L(!0)}}),w.appendChild(this.ollamaCtl.el),t?.addEventListener(`click`,()=>this.cancelEdit()),e.addEventListener(`click`,async()=>{let t=a.value.trim(),n=s.value.trim().replace(/\s+/g,`_`),i=u.value;if(r.classList.add(`hidden`),!t){this.showFormError(`Name is required`);return}if(!n){this.showFormError(`ID is required`);return}let o=this.editing!==null;if(!i&&!o&&v.value.trim()!==`local`){this.showFormError(`Secret value is required`);return}if(!/^[a-zA-Z0-9_-]+$/.test(n)){this.showFormError(`ID can only contain letters, numbers, underscores, and hyphens`);return}e.disabled=!0,e.textContent=`Saving…`;try{let e=v.value.trim(),r=y.value.trim(),a=b.value.trim();await c({id:n,name:t,value:i,cred_type:O,provider:e||void 0,model:r||void 0,base_url:a||void 0},o),this.editing=null,await this.refresh()}catch(t){this.showFormError(`Save failed: ${t}`),e.disabled=!1,e.textContent=this.editing?`Save Changes`:`Save Credential`}})}showFormError(e){let t=this.el.querySelector(`#cred-save-error`);t.textContent=e,t.classList.remove(`hidden`)}bindList(){this.el.querySelectorAll(`.cred-item-edit`).forEach(e=>{e.addEventListener(`click`,()=>this.startEdit(e.dataset.id,e))}),this.el.querySelectorAll(`.cred-item-del`).forEach(e=>{e.addEventListener(`click`,async()=>{if(!this.busy){this.busy=!0;try{let t=e.dataset.id,n=this.creds.find(e=>e.id===t);if(!await p(`Delete credential "${n?.name??t}"? Nodes using it will stop working.`,!0,`Delete`))return;e.disabled=!0,e.textContent=`Deleting…`;try{await r(t),this.editing?.id===t&&(this.editing=null),await this.refresh()}catch(t){e.disabled=!1,e.textContent=`Delete`,await p(`Delete failed: ${t}`)}}finally{this.busy=!1}}})})}async startEdit(e,t){if(this.busy)return;let n=this.creds.find(t=>t.id===e);if(!n)return;this.busy=!0,t.disabled=!0;let r=t.textContent;t.textContent=`Loading…`;try{let t=await f(e).catch(()=>null);this.editing={id:n.id,name:n.name,cred_type:n.cred_type,provider:t?.provider??``,model:t?.model??``,base_url:t?.base_url??``},this.render()}finally{t.disabled=!1,t.textContent=r,this.busy=!1}}cancelEdit(){this.editing=null,this.render()}};async function v(e){try{let t=!0;try{t=await navigator.clipboard.readText()===e}catch{t=!0}t&&await navigator.clipboard.writeText(``)}catch{return}}function y(e){return{api_key:`API Key`,bearer:`Bearer`,basic:`Basic Auth`,oauth:`OAuth`,other:`Custom`}[e]??e}var b=null;function x(e,t,n){b?.(),b=null;let r=e.find(e=>e.value===n)??e[0],i=document.createElement(`div`);i.className=`csel-wrap`;let a=document.createElement(`button`);a.type=`button`,a.className=`csel-trigger`;let o=document.createElement(`span`);o.className=`csel-label`,o.textContent=r.label,o.title=r.label;let s=document.createElement(`span`);s.className=`csel-arrow`,s.innerHTML=`<svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><polyline points="6 9 12 15 18 9"/></svg>`,a.appendChild(o),a.appendChild(s),i.appendChild(a);let c=document.createElement(`div`);c.className=`csel-dropdown hidden`,e.forEach(e=>{let n=document.createElement(`button`);n.type=`button`,n.className=`csel-option`+(e.value===r.value?` selected`:``),n.textContent=e.label,n.addEventListener(`mousedown`,i=>{i.preventDefault(),r=e,o.textContent=e.label,o.title=e.label,c.querySelectorAll(`.csel-option`).forEach(e=>e.classList.remove(`selected`)),n.classList.add(`selected`),c.classList.add(`hidden`),a.setAttribute(`aria-expanded`,`false`),t(e)}),c.appendChild(n)}),i.appendChild(c),a.addEventListener(`click`,()=>{let e=!c.classList.contains(`hidden`);c.classList.toggle(`hidden`,e),a.setAttribute(`aria-expanded`,String(!e))});let l=e=>{i.contains(e.target)||(c.classList.add(`hidden`),a.setAttribute(`aria-expanded`,`false`))};return document.addEventListener(`mousedown`,l),b=()=>document.removeEventListener(`mousedown`,l),i}export{_ as CredentialPanel};