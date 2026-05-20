export function switchTab(name: string): void {
  document.querySelectorAll<HTMLElement>(".panel-tab").forEach(t => {
    t.classList.toggle("active", t.dataset.tab === name);
  });
  document.querySelectorAll<HTMLElement>(".tab-body").forEach(b => {
    b.classList.toggle("active", b.id === `tab-${name}`);
  });
}

export function openPanel(): void  { document.body.classList.add("panel-open"); }
export function closePanel(): void { document.body.classList.remove("panel-open"); }
