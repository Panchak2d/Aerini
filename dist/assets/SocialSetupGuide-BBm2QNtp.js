import{O as e,y as t}from"./index-BEGFpHb5.js";async function n(){return e(`get_oauth_redirect_port`)}var r=42069,i=`http://127.0.0.1:${r}/callback`,a=`%%REDIRECT_URI%%`,o=i,s=!1,c=[{id:`youtube`,label:`YouTube`,icon:`▶`,steps:[`Go to <a href="https://console.cloud.google.com" target="_blank" rel="noopener noreferrer">console.cloud.google.com</a> and create a new project (or select an existing one).`,`In the left menu, go to <strong>APIs &amp; Services → Library</strong>. Search for <strong>YouTube Data API v3</strong> and click <strong>Enable</strong>.`,`Go to <strong>APIs &amp; Services → OAuth consent screen</strong>. Choose <strong>External</strong>, fill in the required fields (App name, support email), and save.`,`Go to <strong>APIs &amp; Services → Credentials</strong>. Click <strong>+ Create Credentials → OAuth client ID</strong>.`,`Set Application type to <strong>Desktop app</strong>. Give it a name and click <strong>Create</strong>.`,`Copy your <strong>Client ID</strong> and <strong>Client Secret</strong> from the dialog.`,`Under <strong>Authorized redirect URIs</strong>, add exactly: <code class="ssg-copyable" data-value="${a}">${a}</code>`,`On the <strong>Social Upload</strong> node, set <strong>Platform</strong> to YouTube and paste your Client ID and Client Secret into the node's own Client ID / Client Secret fields.`],notes:[`Your app starts in 'Testing' mode. Add your Google account as a test user under OAuth consent screen → Test users.`,`To upload publicly without re-authorising every 7 days, submit your app for Google verification (OAuth consent screen → Publish App).`]},{id:`instagram`,label:`Instagram`,icon:`◈`,warning:`Instagram requires media to be hosted at a public URL. The Social Upload node cannot send local files directly to Instagram. Upload your media to a CDN or web server first, then pass the public URL as input.`,steps:[`Go to <a href="https://developers.facebook.com" target="_blank" rel="noopener noreferrer">developers.facebook.com</a> and log in with your Meta account.`,`Click <strong>My Apps → Create App</strong>. Choose <strong>Other</strong> as the use case, then <strong>Consumer</strong> as the app type.`,`Inside your app dashboard, click <strong>Add Product</strong> and add <strong>Instagram</strong>.`,`Go to <strong>Instagram → API setup with Instagram login</strong>. Click <strong>Generate token</strong> to confirm your Instagram account is linked.`,`Go to <strong>App settings → Basic</strong>. Note your <strong>App ID</strong> (Client ID) and <strong>App Secret</strong> (Client Secret).`,`Under <strong>Instagram → Settings → Valid OAuth Redirect URIs</strong>, add exactly: <code class="ssg-copyable" data-value="${a}">${a}</code>`,`Request the <strong>instagram_content_publish</strong> permission under <strong>App Review → Permissions and Features</strong>. For testing, add your Instagram account under <strong>Roles → Instagram Testers</strong>.`,`On the <strong>Social Upload</strong> node, set <strong>Platform</strong> to Instagram and paste your App ID and App Secret into the node's own Client ID / Client Secret fields.`],notes:[`Business or Creator accounts are required for content publishing. Personal accounts are not supported by the Instagram API.`,`The instagram_content_publish permission requires App Review approval before use with accounts other than your own test accounts.`]},{id:`tiktok`,label:`TikTok`,icon:`♪`,steps:[`Go to <a href="https://developers.tiktok.com" target="_blank" rel="noopener noreferrer">developers.tiktok.com</a> and sign in with your TikTok account.`,`Click <strong>Manage Apps → Create app</strong>. Fill in the app name and description. Set platform to <strong>Web</strong>.`,`Under <strong>Products</strong>, add <strong>Login Kit</strong> and <strong>Content Posting API</strong>.`,`In the <strong>Login Kit</strong> settings, add the redirect URI exactly: <code class="ssg-copyable" data-value="${a}">${a}</code>`,`Request the <strong>video.publish</strong> scope under <strong>Content Posting API → Scopes</strong>. For sandbox testing, use the sandbox environment.`,`Copy your <strong>Client Key</strong> (Client ID) and <strong>Client Secret</strong> from the app's <strong>App info</strong> page.`,`On the <strong>Social Upload</strong> node, set <strong>Platform</strong> to TikTok and paste your Client Key and Client Secret into the node's own Client ID / Client Secret fields.`],notes:[`TikTok apps default to sandbox mode. In sandbox, posts are private and visible only to your account. Submit for review to enable public publishing.`,`The video.publish scope requires TikTok app review approval. Processing typically takes 1–3 business days.`]}],l=null,u=`youtube`;function d(){return l||(l=document.createElement(`div`),l.id=`social-setup-guide`,l.className=`ssg-overlay hidden`,document.body.appendChild(l),l)}function f(e=`youtube`){u=e;let t=d();t.classList.remove(`hidden`),m(t),n().then(e=>{let n=`http://127.0.0.1:${e}/callback`,i=e!==r;(n!==o||i!==s)&&(o=n,s=i,t.classList.contains(`hidden`)||m(t))}).catch(()=>{})}function p(){l?.classList.add(`hidden`)}function m(e){let n=t(o),i=e=>e.map(e=>e.split(a).join(n));e.innerHTML=`
    <div class="ssg-backdrop"></div>
    <div class="ssg-box" role="dialog" aria-modal="true" aria-label="Social platform setup guide">
      <div class="ssg-header">
        <div>
          <div class="ssg-title">Social Platform Setup</div>
          <div class="ssg-subtitle">Follow the steps for your platform to create OAuth credentials.</div>
        </div>
        <button class="ssg-close" id="ssg-close" aria-label="Close">
          <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round">
            <line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/>
          </svg>
        </button>
      </div>

      <div class="ssg-tab-bar" role="tablist">
        ${c.map(e=>`
          <button class="ssg-tab${e.id===u?` ssg-tab--active`:``}"
            role="tab"
            aria-selected="${e.id===u}"
            data-platform="${e.id}">
            <span class="ssg-tab-icon">${e.icon}</span>${t(e.label)}
          </button>
        `).join(``)}
      </div>

      <div class="ssg-pane-wrap">
        ${c.map(e=>`
          <div class="ssg-pane${e.id===u?``:` hidden`}" data-pane="${e.id}">
            ${e.warning?`
              <div class="ssg-warning" role="alert">
                <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M10.29 3.86L1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z"/><line x1="12" y1="9" x2="12" y2="13"/><line x1="12" y1="17" x2="12.01" y2="17"/></svg>
                <span>${e.warning}</span>
              </div>
            `:``}
            ${s?`
              <div class="ssg-warning" role="alert">
                <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M10.29 3.86L1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z"/><line x1="12" y1="9" x2="12" y2="13"/><line x1="12" y1="17" x2="12.01" y2="17"/></svg>
                <span>Port ${r} is currently unavailable, so OAuth will use a different port for this session. The redirect URI shown in the steps below and at the bottom of this guide already reflects the port that will actually be used — register that value with this platform, not ${r}, or close whatever else is using port ${r} and reopen this guide.</span>
              </div>
            `:``}
            <ol class="ssg-steps">
              ${i(e.steps).map((e,t)=>`
                <li class="ssg-step">
                  <span class="ssg-step-num">${t+1}</span>
                  <span class="ssg-step-text">${e}</span>
                </li>
              `).join(``)}
            </ol>
            ${e.notes.length?`
              <div class="ssg-notes">
                <div class="ssg-notes-label">Notes</div>
                <ul class="ssg-notes-list">
                  ${e.notes.map(e=>`<li>${t(e)}</li>`).join(``)}
                </ul>
              </div>
            `:``}
          </div>
        `).join(``)}
      </div>

      <div class="ssg-footer">
        <div class="ssg-redirect-row">
          <span class="ssg-redirect-label">Redirect URI (use this exact value in all platforms):</span>
          <span class="ssg-redirect-uri" title="${n}">${n}</span>
          <button class="ssg-copy-btn" id="ssg-copy-uri">Copy</button>
        </div>
      </div>
    </div>`,e.querySelector(`.ssg-backdrop`).addEventListener(`click`,p),e.querySelector(`#ssg-close`).addEventListener(`click`,p),e.querySelectorAll(`.ssg-tab`).forEach(t=>{t.addEventListener(`click`,()=>{u=t.dataset.platform,e.querySelectorAll(`.ssg-tab`).forEach(e=>{e.classList.toggle(`ssg-tab--active`,e===t),e.setAttribute(`aria-selected`,String(e===t))}),e.querySelectorAll(`.ssg-pane`).forEach(e=>{e.classList.toggle(`hidden`,e.dataset.pane!==u)})})}),e.querySelectorAll(`.ssg-copyable`).forEach(e=>{e.title=`Click to copy`,e.setAttribute(`data-tooltip`,`Click to copy`),e.style.cursor=`pointer`,e.addEventListener(`click`,()=>{navigator.clipboard.writeText(e.dataset.value??e.textContent??``).catch(()=>{});let t=e.textContent;e.textContent=`Copied!`,setTimeout(()=>{e.textContent=t},1500)})}),e.querySelector(`#ssg-copy-uri`).addEventListener(`click`,e=>{let t=e.currentTarget;navigator.clipboard.writeText(o).catch(()=>{}),t.textContent=`Copied!`,setTimeout(()=>{t.textContent=`Copy`},1500)})}export{f as showSocialSetupGuide};