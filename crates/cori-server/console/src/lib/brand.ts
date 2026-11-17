// The deployment's brand: the server writes it into the page before it loads (window.__CORI_BRAND__, from its brand
// file); without one, Cori's. Images are data: or https: URLs.
import logo from "../assets/cori-logo.png";
import logoWhite from "../assets/cori-logo-white.png";
import mark from "../assets/cori-mark.png";

type Injected = { name?: string; logo?: string; logo_dark?: string; mark?: string };
const injected: Injected = (window as { __CORI_BRAND__?: Injected }).__CORI_BRAND__ ?? {};

export const brand = {
  /** Another brand than Cori's: the console then says it is powered by Cori. */
  custom: Boolean((injected.name && injected.name !== "Cori") || injected.logo || injected.logo_dark || injected.mark),
  /** Cori's own mark, for that line. */
  coriMark: mark,
  name: injected.name || "Cori",
  logo: injected.logo || logo,
  // a brand with one logo shows it in both themes
  logoDark: injected.logo_dark || injected.logo || logoWhite,
  mark: injected.mark || injected.logo || mark,
};
