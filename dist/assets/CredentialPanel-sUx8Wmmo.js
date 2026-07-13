import{a as e,d as t,i as n,l as r,o as i}from"./index-B6c7Lvf8.js";var a=class{el;creds=[];constructor(){this.el=document.getElementById(`cred-panel`),this.el||(this.el=document.createElement(`div`),this.el.id=`cred-panel`,this.el.className=`cred-panel hidden`,document.body.appendChild(this.el))}async show(){this.el.classList.remove(`hidden`),await this.refresh()}hide(){this.el.classList.add(`hidden`)}async refresh(){this.creds=await e().catch(()=>[]),this.render()}render(){let e=this.creds.length===0;this.el.innerHTML=`
      <div class="cred-panel-backdrop"></div>
      <div class="cred-panel-box">
        <div class="cred-panel-header">
          <div>
            <div class="cred-panel-title">Credentials</div>
            <div class="cred-panel-subtitle">API keys and service credentials — stored encrypted on your device. Never sent anywhere.</div>
          </div>
          <button class="cred-panel-close" id="cred-close">
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round">
              <line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/>
            </svg>
          </button>
        </div>

        ${e?`
        <div class="cred-empty-state">
          <svg width="32" height="32" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" opacity="0.4"><path d="M10 13a5 5 0 0 0 7.54.54l3-3a5 5 0 0 0-7.07-7.07l-1.72 1.71"/><path d="M14 11a5 5 0 0 0-7.54-.54l-3 3a5 5 0 0 0 7.07 7.07l1.71-1.71"/></svg>
          <div class="cred-empty-state-title">No credentials yet</div>
          <div class="cred-empty-state-desc">Add your first API key or service credential below. Credentials are used by nodes like HTTP Request, AI Prompt, and Send Email.</div>
        </div>`:`
        <div class="cred-list" id="cred-list">
          ${this.renderList()}
        </div>`}

        <div class="cred-add-section">
          <div class="cred-add-header">
            <div class="cred-add-title">${e?`Add your first credential`:`Add a credential`}</div>
          </div>
          <div class="cred-form" id="cred-form">
            ${this.renderForm()}
          </div>
        </div>
      </div>`,this.el.querySelector(`#cred-close`).addEventListener(`click`,()=>this.hide()),this.el.querySelector(`.cred-panel-backdrop`).addEventListener(`click`,()=>this.hide()),this.bindForm(),this.bindList()}renderList(){return this.creds.length?this.creds.map(e=>`
      <div class="cred-item" data-id="${t(e.id)}">
        <div class="cred-item-icon"><svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><circle cx="7.5" cy="15.5" r="3.5"/><path d="M21 2l-9.6 9.6"/><path d="M15.5 7.5l3 3L22 7l-3-3"/></svg></div>
        <div class="cred-item-info">
          <div class="cred-item-name">${t(e.name)}</div>
          <div class="cred-item-id">${t(e.id)}</div>
        </div>
        <span class="cred-item-type">${t(o(e.cred_type))}</span>
        <button class="cred-item-del" data-id="${t(e.id)}" title="Delete this credential">Delete</button>
      </div>`).join(``):`<div class="cred-empty">
        No credentials saved yet. Add your first API key below.
      </div>`}renderForm(){return`
      <div class="field-group">
        <label class="field-label">Type</label>
        <div id="cred-type-wrap"></div>
        <div class="field-hint" id="cred-type-hint">A plain API key passed as a header or query param</div>
      </div>
      <div class="field-group">
        <label class="field-label">Name</label>
        <input id="cred-name" type="text" placeholder="e.g. OpenAI Production Key" autocomplete="off" />
        <div class="field-hint">A label to identify this credential in the UI</div>
      </div>
      <div class="field-group">
        <label class="field-label">ID / Key</label>
        <input id="cred-id" type="text" placeholder="e.g. openai_prod (no spaces)" autocomplete="off" />
        <div class="field-hint">Short identifier used in nodes — auto-filled from name</div>
      </div>
      <div class="field-group">
        <label class="field-label">Secret Value</label>
        <div class="cred-secret-wrap">
          <input id="cred-value" type="password" placeholder="sk-… or your API key" autocomplete="new-password" />
          <button type="button" class="cred-show-btn" id="cred-show" title="Show / hide">
            <svg id="cred-eye-icon" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round">
              <path d="M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z"/><circle cx="12" cy="12" r="3"/>
            </svg>
          </button>
        </div>
        <div class="field-hint">Stored encrypted on your device. Never sent to Aerini servers.</div>
      </div>
      <details class="cred-advanced">
        <summary class="cred-advanced-summary">Advanced (optional) — provider, model, base URL</summary>
        <div class="field-group">
          <label class="field-label">Provider</label>
          <input id="cred-meta-provider" type="text" placeholder="e.g. openai, anthropic, gemini" autocomplete="off" />
        </div>
        <div class="field-group">
          <label class="field-label">Model</label>
          <input id="cred-meta-model" type="text" placeholder="e.g. gpt-4o, claude-sonnet-4-6" autocomplete="off" />
        </div>
        <div class="field-group">
          <label class="field-label">Base URL</label>
          <input id="cred-meta-base-url" type="text" placeholder="Leave blank for provider default" autocomplete="off" />
        </div>
        <div class="field-hint">Not secret — used to auto-fill matching fields on AI nodes when this credential is selected.</div>
      </details>
      <div class="cred-form-actions">
        <div class="cred-save-error hidden" id="cred-save-error"></div>
        <button class="btn-primary cred-save-btn" id="cred-save">Save Credential</button>
      </div>`}bindForm(){let e=this.el.querySelector(`#cred-save`),t=this.el.querySelector(`#cred-save-error`),n=this.el.querySelector(`#cred-name`),r=this.el.querySelector(`#cred-id`),a=this.el.querySelector(`#cred-value`),o=this.el.querySelector(`#cred-type-hint`),c=this.el.querySelector(`#cred-type-wrap`),l=this.el.querySelector(`#cred-show`),u=this.el.querySelector(`#cred-eye-icon`),d=this.el.querySelector(`#cred-meta-provider`),f=this.el.querySelector(`#cred-meta-model`),p=this.el.querySelector(`#cred-meta-base-url`),m=[{value:`api_key`,label:`API Key`,hint:`A plain API key passed as a header or query param`,placeholder:`sk-… or your API key`},{value:`bearer`,label:`Bearer Token`,hint:`Will be sent as Authorization: Bearer <value>`,placeholder:`eyJ… or your token`},{value:`basic`,label:`Basic Auth`,hint:`Enter as username:password`,placeholder:`username:password`},{value:`oauth`,label:`OAuth Token`,hint:`An OAuth access or refresh token`,placeholder:`ya29.… or your OAuth token`},{value:`other`,label:`Other / Custom`,hint:`Any custom secret value`,placeholder:`Your secret value`}],h=m[0].value;c.appendChild(s(m,e=>{o.textContent=e.hint,a.placeholder=e.placeholder,h=e.value}));let g=!1;r.addEventListener(`input`,()=>{g=!0}),n.addEventListener(`input`,()=>{g||(r.value=n.value.trim().toLowerCase().replace(/[^a-z0-9]+/g,`_`).replace(/^_+|_+$/g,``).slice(0,40))}),l.addEventListener(`click`,()=>{let e=a.type===`text`;a.type=e?`password`:`text`,u.innerHTML=e?`<path d="M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z"/><circle cx="12" cy="12" r="3"/>`:`<path d="M17.94 17.94A10.07 10.07 0 0 1 12 20c-7 0-11-8-11-8a18.45 18.45 0 0 1 5.06-5.94"/><path d="M9.9 4.24A9.12 9.12 0 0 1 12 4c7 0 11 8 11 8a18.5 18.5 0 0 1-2.16 3.19"/><line x1="1" y1="1" x2="23" y2="23"/>`}),e.addEventListener(`click`,async()=>{let o=n.value.trim(),s=r.value.trim().replace(/\s+/g,`_`),c=a.value;if(t.classList.add(`hidden`),!o){this.showFormError(`Name is required`);return}if(!s){this.showFormError(`ID is required`);return}if(!c){this.showFormError(`Secret value is required`);return}if(!/^[a-zA-Z0-9_-]+$/.test(s)){this.showFormError(`ID can only contain letters, numbers, underscores, and hyphens`);return}e.disabled=!0,e.textContent=`Saving…`;try{let e=d.value.trim(),t=f.value.trim(),n=p.value.trim();await i({id:s,name:o,value:c,cred_type:h,provider:e||void 0,model:t||void 0,base_url:n||void 0}),await this.refresh()}catch(t){this.showFormError(`Save failed: ${t}`),e.disabled=!1,e.textContent=`Save Credential`}})}showFormError(e){let t=this.el.querySelector(`#cred-save-error`);t.textContent=e,t.classList.remove(`hidden`)}bindList(){this.el.querySelectorAll(`.cred-item-del`).forEach(e=>{e.addEventListener(`click`,async()=>{let t=e.dataset.id;if(await r(`Delete credential "${this.creds.find(e=>e.id===t)?.name??t}"? Nodes using it will stop working.`,!0,`Delete`)){e.disabled=!0,e.textContent=`Deleting…`;try{await n(t),await this.refresh()}catch(t){e.disabled=!1,e.textContent=`Delete`,await r(`Delete failed: ${t}`)}}})})}};function o(e){return{api_key:`API Key`,bearer:`Bearer`,basic:`Basic Auth`,oauth:`OAuth`,other:`Custom`}[e]??e}function s(e,t){let n=e[0],r=document.createElement(`div`);r.className=`csel-wrap`;let i=document.createElement(`button`);i.type=`button`,i.className=`csel-trigger`;let a=document.createElement(`span`);a.className=`csel-label`,a.textContent=n.label;let o=document.createElement(`span`);o.className=`csel-arrow`,o.innerHTML=`<svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round"><polyline points="6 9 12 15 18 9"/></svg>`,i.appendChild(a),i.appendChild(o),r.appendChild(i);let s=document.createElement(`div`);return s.className=`csel-dropdown hidden`,e.forEach(e=>{let r=document.createElement(`button`);r.type=`button`,r.className=`csel-option`+(e.value===n.value?` selected`:``),r.textContent=e.label,r.addEventListener(`mousedown`,o=>{o.preventDefault(),n=e,a.textContent=e.label,s.querySelectorAll(`.csel-option`).forEach(e=>e.classList.remove(`selected`)),r.classList.add(`selected`),s.classList.add(`hidden`),i.setAttribute(`aria-expanded`,`false`),t(e)}),s.appendChild(r)}),r.appendChild(s),i.addEventListener(`click`,()=>{let e=!s.classList.contains(`hidden`);s.classList.toggle(`hidden`,e),i.setAttribute(`aria-expanded`,String(!e))}),document.addEventListener(`mousedown`,e=>{r.contains(e.target)||(s.classList.add(`hidden`),i.setAttribute(`aria-expanded`,`false`))}),r}export{a as CredentialPanel};