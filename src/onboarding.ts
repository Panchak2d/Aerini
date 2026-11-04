import { storageGet, storageSet } from "./safe-storage";
import { getTemplates } from "./templates";
import { renderTemplateCards, type TemplateLoader } from "./template-gallery";

export const ONBOARDED_KEY = "aerini_onboarded";

export function initOnboarding(loadTemplate: TemplateLoader): void {
  const modal   = document.getElementById("onboarding-modal")!;
  const dismiss = document.getElementById("onboarding-dismiss")!;
  const closeX  = document.getElementById("onboarding-close");
  const gallery = document.getElementById("onboarding-templates");

  if (storageGet(ONBOARDED_KEY)) return;

  const onKeydown = (e: KeyboardEvent) => {
    if (e.key !== "Escape") return;
    e.preventDefault();
    e.stopPropagation();
    close();
  };

  const close = () => {
    document.removeEventListener("keydown", onKeydown, true);
    modal.classList.add("hidden");
    storageSet(ONBOARDED_KEY, "1");
  };

  if (gallery) {
    renderTemplateCards(gallery, getTemplates(), (t) => {
      close();
      void loadTemplate(t.workflow);
    });
  }

  setTimeout(() => {
    modal.classList.remove("hidden");
    document.addEventListener("keydown", onKeydown, true);
    dismiss.focus({ preventScroll: true });
  }, 400);

  dismiss.addEventListener("click", close);
  closeX?.addEventListener("click", close);
  modal.addEventListener("mousedown", (e) => {
    if (e.target === modal) close();
  });
}
