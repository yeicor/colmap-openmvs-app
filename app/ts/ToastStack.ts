/**
 * Toast notification stack for the 3D viewer.
 */
export class ToastStack {
  private el: HTMLDivElement;

  constructor(root: HTMLElement) {
    this.el = document.createElement("div");
    this.el.className = "v3d-toast-stack";
    root.appendChild(this.el);
  }

  push(title: string, html: string): HTMLDivElement {
    const card = document.createElement("div");
    card.className = "v3d-toast";
    card.innerHTML = `
      <div class="v3d-toast-header">
        <div>${title}</div>
        <button>×</button>
      </div>
      <div class="v3d-toast-content">${html}</div>`;
    card.querySelector("button")!.onclick = () => card.remove();

    let timer = setTimeout(() => card.remove(), 10000);
    card.addEventListener("pointerenter", () => clearTimeout(timer));
    card.addEventListener("pointerleave", () => {
      timer = setTimeout(() => card.remove(), 10000);
    });
    this.el.appendChild(card);
    return card;
  }
}
