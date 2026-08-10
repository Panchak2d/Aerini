import{a as e,c as t,d as n,f as r,i,o as a,s as o}from"./index-Bgcm_eAN.js";var s=[{value:`api_key`,label:`API Key`,hint:`A plain API key passed as a header or query param`,placeholder:`sk-… or your API key`},{value:`bearer`,label:`Bearer Token`,hint:`Will be sent as Authorization: Bearer <value>`,placeholder:`eyJ… or your token`},{value:`basic`,label:`Basic Auth`,hint:`Enter as username:password`,placeholder:`username:password`},{value:`oauth`,label:`OAuth Token`,hint:`An OAuth access or refresh token`,placeholder:`ya29.… or your OAuth token`},{value:`other`,label:`Other / Custom`,hint:`Any custom secret value`,placeholder:`Your secret value`}],c=class{el;creds=[];editing=null;busy=!1;constructor(){this.el=document.getElementById(`cred-panel`),this.el||(this.el=document.createElement(`div`),this.el.id=`cred-panel`,this.el.className=`cred-panel hidden`,document.body.appendChild(this.el))}async show(){this.editing=null,this.el.classList.remove(`hidden`),await this.refresh()}hide(){this.el.classList.add(`hidden`)}async refresh(){this.creds=await o().catch(()=>[]),this.render()}render(){let e=this.creds.length===0;this.el.innerHTML=`
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
      </div>`,this.el.querySelector(`#cred-close`).addEventListener(`click`,()=>this.hide()),this.el.querySelector(`.cred-panel-backdrop`).addEventListener(`click`,()=>this.hide()),this.bindForm(),this.bindList()}formTitle(){return this.editing?`Editing — ${r(this.editing.name)}`:this.creds.length===0?`Add your first credential`:`Add a credential`}renderList(){return this.creds.length?this.creds.map(e=>`
      <div class="cred-item${this.editing?.id===e.id?` editing`:``}" data-id="${r(e.id)}">
        <div class="cred-item-icon"><svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><circle cx="7.5" cy="15.5" r="3.5"/><path d="M21 2l-9.6 9.6"/><path d="M15.5 7.5l3 3L22 7l-3-3"/></svg></div>
        <div class="cred-item-info">
          <div class="cred-item-name">${r(e.name)}</div>
          <div class="cred-item-id">${r(e.id)}</div>
        </div>
        <span class="cred-item-type">${r(l(e.cred_type))}</span>
        <div class="cred-item-actions">
          <button class="cred-item-edit" data-id="${r(e.id)}" title="View / edit this credential">Edit</button>
          <button class="cred-item-del" data-id="${r(e.id)}" title="Delete this credential">Delete</button>
        </div>
      </div>`).join(``):`<div class="cred-empty">
        No credentials saved yet. Add your first API key below.
      </div>`}renderForm(){let e=this.editing,t=s.find(t=>t.value===e?.cred_type)??s[0];return`
      <div class="field-group">
        <label class="field-label">Type</label>
        <div id="cred-type-wrap"></div>
        <div class="field-hint" id="cred-type-hint">${r(t.hint)}</div>
      </div>
      <div class="field-group">
        <label class="field-label">Name</label>
        <input id="cred-name" type="text" placeholder="e.g. OpenAI Production Key" autocomplete="off" value="${r(e?.name??``)}" />
        <div class="field-hint">A label to identify this credential in the UI</div>
      </div>
      <div class="field-group">
        <label class="field-label">ID / Key</label>
        <input id="cred-id" type="text" placeholder="e.g. openai_prod (no spaces)" autocomplete="off" value="${r(e?.id??``)}" ${e?`readonly`:``} />
        <div class="field-hint">${e?`ID can't be changed after creation — delete and re-add to use a different one`:`Short identifier used in nodes — auto-filled from name`}</div>
      </div>
      <div class="field-group">
        <label class="field-label">Secret Value</label>
        <div class="cred-secret-wrap">
          <input id="cred-value" type="password" placeholder="${r(t.placeholder)}" autocomplete="new-password" value="${r(e?.value??``)}" />
          <button type="button" class="cred-show-btn" id="cred-show" title="Show / hide" aria-label="Show or hide secret value">
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
          <input id="cred-meta-provider" type="text" placeholder="e.g. openai, anthropic, gemini" autocomplete="off" value="${r(e?.provider??``)}" />
        </div>
        <div class="field-group">
          <label class="field-label">Model</label>
          <input id="cred-meta-model" type="text" placeholder="e.g. gpt-4o, claude-sonnet-4-6" autocomplete="off" value="${r(e?.model??``)}" />
        </div>
        <div class="field-group">
          <label class="field-label">Base URL</label>
          <input id="cred-meta-base-url" type="text" placeholder="Leave blank for provider default" autocomplete="off" value="${r(e?.base_url??``)}" />
        </div>
        <div class="field-hint">Not secret — used to auto-fill matching fields on AI nodes when this credential is selected.</div>
      </details>`}bindForm(){let e=this.el.querySelector(`#cred-save`),n=this.el.querySelector(`#cred-cancel`),r=this.el.querySelector(`#cred-save-error`),i=this.el.querySelector(`#cred-name`),a=this.el.querySelector(`#cred-id`),o=this.el.querySelector(`#cred-value`),c=this.el.querySelector(`#cred-type-hint`),l=this.el.querySelector(`#cred-type-wrap`),u=this.el.querySelector(`#cred-show`),f=this.el.querySelector(`#cred-eye-icon`),p=this.el.querySelector(`#cred-meta-provider`),m=this.el.querySelector(`#cred-meta-model`),h=this.el.querySelector(`#cred-meta-base-url`),g=this.editing?.cred_type??s[0].value;l.appendChild(d(s,e=>{c.textContent=e.hint,o.placeholder=e.placeholder,g=e.value},g));let _=!!this.editing;a.addEventListener(`input`,()=>{_=!0}),i.addEventListener(`input`,()=>{_||(a.value=i.value.trim().toLowerCase().replace(/[^a-z0-9]+/g,`_`).replace(/^_+|_+$/g,``).slice(0,40))}),u.addEventListener(`click`,()=>{let e=o.type===`text`;o.type=e?`password`:`text`,f.innerHTML=e?`<path d="M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z"/><circle cx="12" cy="12" r="3"/>`:`<path d="M17.94 17.94A10.07 10.07 0 0 1 12 20c-7 0-11-8-11-8a18.45 18.45 0 0 1 5.06-5.94"/><path d="M9.9 4.24A9.12 9.12 0 0 1 12 4c7 0 11 8 11 8a18.5 18.5 0 0 1-2.16 3.19"/><line x1="1" y1="1" x2="23" y2="23"/>`}),n?.addEventListener(`click`,()=>this.cancelEdit()),e.addEventListener(`click`,async()=>{let n=i.value.trim(),s=a.value.trim().replace(/\s+/g,`_`),c=o.value;if(r.classList.add(`hidden`),!n){this.showFormError(`Name is required`);return}if(!s){this.showFormError(`ID is required`);return}if(!c){this.showFormError(`Secret value is required`);return}if(!/^[a-zA-Z0-9_-]+$/.test(s)){this.showFormError(`ID can only contain letters, numbers, underscores, and hyphens`);return}e.disabled=!0,e.textContent=`Saving…`;try{let e=p.value.trim(),r=m.value.trim(),i=h.value.trim();await t({id:s,name:n,value:c,cred_type:g,provider:e||void 0,model:r||void 0,base_url:i||void 0}),this.editing=null,await this.refresh()}catch(t){this.showFormError(`Save failed: ${t}`),e.disabled=!1,e.textContent=this.editing?`Save Changes`:`Save Credential`}})}showFormError(e){let t=this.el.querySelector(`#cred-save-error`);t.textContent=e,t.classList.remove(`hidden`)}bindList(){this.el.querySelectorAll(`.cred-item-edit`).forEach(e=>{e.addEventListener(`click`,()=>this.startEdit(e.dataset.id,e))}),this.el.querySelectorAll(`.cred-item-del`).forEach(e=>{e.addEventListener(`click`,async()=>{if(!this.busy){this.busy=!0;try{let t=e.dataset.id,r=this.creds.find(e=>e.id===t);if(!await n(`Delete credential "${r?.name??t}"? Nodes using it will stop working.`,!0,`Delete`))return;e.disabled=!0,e.textContent=`Deleting…`;try{await i(t),this.editing?.id===t&&(this.editing=null),await this.refresh()}catch(t){e.disabled=!1,e.textContent=`Delete`,await n(`Delete failed: ${t}`)}}finally{this.busy=!1}}})})}async startEdit(t,r){if(this.busy)return;let i=this.creds.find(e=>e.id===t);if(!i)return;this.busy=!0,r.disabled=!0;let o=r.textContent;r.textContent=`Loading…`;try{let[s,c]=await Promise.all([a(t),e(t).catch(()=>null)]);if(s==null){r.disabled=!1,r.textContent=o,await n(`Could not load credential "${i.name}" — it may have been deleted.`),await this.refresh();return}this.editing={id:i.id,name:i.name,cred_type:i.cred_type,value:s,provider:c?.provider??``,model:c?.model??``,base_url:c?.base_url??``},this.render()}catch(e){r.disabled=!1,r.textContent=o,await n(`Failed to load credential: ${e}`)}finally{this.busy=!1}}cancelEdit(){this.editing=null,this.render()}};function l(e){return{api_key:`API Key`,bearer:`Bearer`,basic:`Basic Auth`,oauth:`OAuth`,other:`Custom`}[e]??e}var u=null;function d(e,t,n){u?.(),u=null;let r=e.find(e=>e.value===n)??e[0],i=document.createElement(`div`);i.className=`csel-wrap`;let a=document.createElement(`button`);a.type=`button`,a.className=`csel-trigger`;let o=document.createElement(`span`);o.className=`csel-label`,o.textContent=r.label;let s=document.createElement(`span`);s.className=`csel-arrow`,s.innerHTML=`<svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><polyline points="6 9 12 15 18 9"/></svg>`,a.appendChild(o),a.appendChild(s),i.appendChild(a);let c=document.createElement(`div`);c.className=`csel-dropdown hidden`,e.forEach(e=>{let n=document.createElement(`button`);n.type=`button`,n.className=`csel-option`+(e.value===r.value?` selected`:``),n.textContent=e.label,n.addEventListener(`mousedown`,i=>{i.preventDefault(),r=e,o.textContent=e.label,c.querySelectorAll(`.csel-option`).forEach(e=>e.classList.remove(`selected`)),n.classList.add(`selected`),c.classList.add(`hidden`),a.setAttribute(`aria-expanded`,`false`),t(e)}),c.appendChild(n)}),i.appendChild(c),a.addEventListener(`click`,()=>{let e=!c.classList.contains(`hidden`);c.classList.toggle(`hidden`,e),a.setAttribute(`aria-expanded`,String(!e))});let l=e=>{i.contains(e.target)||(c.classList.add(`hidden`),a.setAttribute(`aria-expanded`,`false`))};return document.addEventListener(`mousedown`,l),u=()=>document.removeEventListener(`mousedown`,l),i}export{c as CredentialPanel};