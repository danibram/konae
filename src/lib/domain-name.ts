// The hostname rules for a custom domain (ADR-0015). The Kernel's
// kernel-rs/src/domain.rs applies the same rules with the same words, and both
// test suites run kernel-rs/src/domain-cases.json.

export type DomainRules = {
  /** The installation's app suffix, LITTLEBOX_HOST_SUFFIX. */
  suffix: string;
  /** The hosts of LITTLEBOX_PUBLIC_ORIGIN and LITTLEBOX_MCP_ORIGIN. */
  installationHosts: string[];
};

export type HostnameCheck = { ok: true; hostname: string } | { ok: false; message: string };

/** The TXT record name that proves control of a hostname. */
export const domainRecordName = (hostname: string) => `_littlebox.${hostname}`;

export function checkHostname(raw: string, rules: DomainRules): HostnameCheck {
  const trimmed = raw.trim();
  const hostname = (trimmed.endsWith(".") ? trimmed.slice(0, -1) : trimmed).toLowerCase();
  const refuse = (message: string): HostnameCheck => ({ ok: false, message });
  if (!hostname) return refuse("Name a hostname, such as notes.example.org.");
  // The Kernel counts bytes; a name with a non-ASCII character is refused below either way.
  if (new TextEncoder().encode(hostname).length > 253) return refuse("A hostname has at most 253 characters.");
  const labels = hostname.split(".");
  const validLabel = (label: string) => label.length >= 1 && label.length <= 63 && /^[a-z0-9-]+$/.test(label) && !label.startsWith("-") && !label.endsWith("-");
  if (!labels.every(validLabel)) {
    return refuse(`${hostname} is not a valid DNS name. Use labels of letters, digits and hyphens, 1 to 63 characters each, that do not start or end with a hyphen.`);
  }
  if (labels.length < 2) return refuse(`${hostname} needs at least two labels, such as notes.example.org.`);
  if (/^[0-9]+$/.test(labels[labels.length - 1])) return refuse(`${hostname} is an IP address. Name a hostname under a domain you control.`);
  const suffix = rules.suffix.replace(/^\.+/, "").toLowerCase();
  if (suffix && (hostname === suffix || hostname.endsWith(`.${suffix}`))) {
    return refuse(`${hostname} is under this installation's app suffix ${suffix}, where Littlebox names apps itself. Name a hostname outside it.`);
  }
  if (hostname === "localhost" || hostname.endsWith(".localhost")) return refuse(`${hostname} is a local name. Name a hostname under a domain you control.`);
  if (rules.installationHosts.some((host) => host.toLowerCase() === hostname)) {
    return refuse(`${hostname} is this installation's own host. Name a hostname no Littlebox host uses.`);
  }
  return { ok: true, hostname };
}
