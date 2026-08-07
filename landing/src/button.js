const finePointer = window.matchMedia("(hover: hover) and (pointer: fine)");

for (const action of document.querySelectorAll(".primary-action")) {
  action.addEventListener("pointermove", (event) => {
    if (!finePointer.matches) return;

    const bounds = action.getBoundingClientRect();
    const x = Math.min(Math.max(event.clientX - bounds.left, 0), bounds.width);
    action.style.setProperty("--pointer-x", `${(x / bounds.width) * 100}%`);
  });

  action.addEventListener("pointerleave", () => {
    action.style.setProperty("--pointer-x", "50%");
  });
}
