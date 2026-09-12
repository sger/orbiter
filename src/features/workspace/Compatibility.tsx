import { Check, Info, Orbit, ShieldCheck, X } from "lucide-react";
import type { Preparation, Report } from "../../types";
import { date } from "./format";
const labels = {
  not_verified: "Not yet verified",
  requires_configuration: "Configuration required",
  unsupported: "Unsupported",
  preserved: "Supported & preserved",
};
/// Findings the prepared team actually establishes. Once Apple has answered, "not yet verified"
/// is no longer true of these capabilities: they are decided, and the decision is what to show.
function preparedFindings(preparation: Preparation) {
  const findings: {
    status: keyof typeof labels;
    title: string;
    /// Overrides the status word when the status label would misdescribe a settled fact.
    label?: string;
    detail: string;
    bundle: string | null;
  }[] = [];
  const plan = preparation.plan;
  if (plan.blockers.length) {
    for (const blocker of plan.blockers)
      findings.push({
        status: "unsupported",
        title: "Re-signing blocked",
        detail: blocker,
        bundle: null,
      });
    return findings;
  }
  const expiry = preparation.profiles
    .map((profile) => profile.expires)
    .sort()[0];
  findings.push({
    status: "preserved",
    title: "Identifiers and profiles prepared",
    detail: `Apple registered ${preparation.app_ids.length} identifier(s) for this team and returned their profiles${
      expiry ? `, valid until ${date(expiry)}` : ""
    }. The build will install under a rewritten identifier, not the company one.`,
    bundle: plan.new_main_identifier,
  });
  // One finding per capability the team could not carry, stated as decided rather than unknown.
  const removed = new Map<string, { detail: string; bundle: string }>();
  const removedConsequences = new Set<string>();
  for (const bundle of plan.bundles)
    for (const capability of bundle.capabilities)
      if (capability.action === "remove" && capability.consequence) {
        removed.set(capability.key, {
          detail: capability.consequence,
          bundle: bundle.new_identifier,
        });
        removedConsequences.add(capability.consequence);
      }
  for (const [key, entry] of removed)
    findings.push({
      status: "unsupported",
      title: capabilityTitle(key),
      detail: entry.detail,
      bundle: entry.bundle,
    });
  // Consequences the whole plan carries — the seven-day expiry, and what was decided about a
  // Watch app. These are established facts about the prepared build, not open configuration.
  for (const consequence of plan.consequences)
    if (!removedConsequences.has(consequence))
      findings.push({
        status: "requires_configuration",
        title: consequence.includes("seven days")
          ? "Weekly re-signing"
          : consequence.includes("bundle identifier")
            ? "Services that know the old identifier"
            : "Watch app",
        label: consequence.includes("seven days")
          ? "Every seven days"
          : consequence.includes("bundle identifier")
            ? "Needs registering"
            : consequence.includes("removed")
              ? "Removed by your choice"
              : "Kept, unverified",
        detail: consequence,
        bundle: null,
      });
  return findings;
}
function capabilityTitle(key: string) {
  const titles: Record<string, string> = {
    "aps-environment": "Push notifications",
    "com.apple.developer.aps-environment": "Push notifications",
    "com.apple.developer.associated-domains": "Associated domains",
    "com.apple.developer.in-app-payments": "Apple Pay",
    "com.apple.security.application-groups": "App groups",
  };
  return titles[key] ?? key;
}
export function Compatibility({
  report,
  preparation,
}: {
  report: Report | null;
  preparation: Preparation | null;
}) {
  return (
    <section
      className={`card compatibility ${!report ? "compatibility-empty" : ""}`}
    >
      <div className="section-heading">
        <h2>
          <span>03</span> Compatibility
        </h2>
        <ShieldCheck size={18} />
      </div>
      <div className="assessment-label">
        <i className={report ? "amber" : ""} />
        {report ? "Review required" : "Awaiting application"}
      </div>
      {report ? (
        <>
          <h3>Know what needs attention.</h3>
          <p className="assessment-intro">
            {preparation
              ? "What the prepared team establishes for this build. Runtime behavior is still not verified."
              : "Static findings from this build. Target-team compatibility and runtime behavior are not verified."}
          </p>
          <div className="findings">
            {(preparation
              ? preparedFindings(preparation)
              : report.findings
            ).map((f, i) => (
              <article className={`finding ${f.status}`} key={i}>
                <div className="finding-icon">
                  {f.status === "unsupported" ? (
                    <X size={15} />
                  ) : (
                    <Info size={15} />
                  )}
                </div>
                <div>
                  <div className="finding-heading">
                    <h4>{f.title}</h4>
                    <span>
                      {(f as { label?: string }).label ?? labels[f.status]}
                    </span>
                  </div>
                  <p>{f.detail}</p>
                  {f.bundle && <code>{f.bundle}</code>}
                </div>
              </article>
            ))}
          </div>
        </>
      ) : (
        <div className="empty-assessment">
          <div className="orbit-art">
            <Orbit size={58} strokeWidth={1} />
            <div />
          </div>
          <p>
            Add an IPA to review its provisioning, embedded apps, and signing
            capabilities.
          </p>
          <ul>
            <li>
              <Check size={14} /> Executable & encryption checks
            </li>
            <li>
              <Check size={14} /> Profiles & capability inventory
            </li>
            <li>
              <Check size={14} /> Watch & extension discovery
            </li>
          </ul>
        </div>
      )}
      <div className="compatibility-footer">
        <Info size={15} />
        <span>
          Installing successfully does not prove the app's features work
          correctly.
        </span>
      </div>
    </section>
  );
}
