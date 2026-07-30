const root = document.documentElement;
const header = document.querySelector(".site-header");
const reveals = [...document.querySelectorAll(".reveal")];
const navLinks = [...document.querySelectorAll(".desktop-nav a[href^='#']")];
const sections = navLinks
  .map((link) => document.querySelector(link.getAttribute("href")))
  .filter(Boolean);

const reduceMotion = window.matchMedia(
  "(prefers-reduced-motion: reduce)",
).matches;

if (!reduceMotion && "IntersectionObserver" in window) {
  root.classList.add("motion-ready");
  const revealObserver = new IntersectionObserver(
    (entries, observer) => {
      for (const entry of entries) {
        if (entry.isIntersecting) {
          entry.target.classList.add("is-visible");
          observer.unobserve(entry.target);
        }
      }
    },
    { rootMargin: "0px 0px -8% 0px", threshold: 0.08 },
  );
  reveals.forEach((element) => revealObserver.observe(element));
} else {
  reveals.forEach((element) => element.classList.add("is-visible"));
}

const updateHeader = () =>
  header?.classList.toggle("is-scrolled", window.scrollY > 8);
updateHeader();
window.addEventListener("scroll", updateHeader, { passive: true });

if ("IntersectionObserver" in window) {
  const sectionObserver = new IntersectionObserver(
    (entries) => {
      const current = entries
        .filter((entry) => entry.isIntersecting)
        .sort((a, b) => b.intersectionRatio - a.intersectionRatio)[0];
      if (!current) return;
      navLinks.forEach((link) => {
        const active = link.getAttribute("href") === `#${current.target.id}`;
        link.classList.toggle("is-active", active);
        if (active) link.setAttribute("aria-current", "location");
        else link.removeAttribute("aria-current");
      });
    },
    { rootMargin: "-20% 0px -65% 0px", threshold: [0, 0.2, 0.6] },
  );
  sections.forEach((section) => sectionObserver.observe(section));
}

document.querySelectorAll(".copy-button").forEach((button) => {
  button.addEventListener("click", async () => {
    const original = button.textContent;
    try {
      await navigator.clipboard.writeText(button.dataset.copy ?? "");
      button.textContent = "Copied";
    } catch {
      button.textContent = "Select";
    }
    window.setTimeout(() => {
      button.textContent = original;
    }, 1400);
  });
});

document.querySelectorAll(".mobile-nav a").forEach((link) => {
  link.addEventListener("click", () =>
    link.closest("details")?.removeAttribute("open"),
  );
});

document.querySelector("#year").textContent = String(new Date().getFullYear());
