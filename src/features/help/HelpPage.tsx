import { HelpContent } from "./HelpContent";

export function HelpPage() {
  return (
    <section aria-label="Help">
      <h1 className="page-title" tabIndex={-1}>
        Help
      </h1>
      <div className="card help-body help-page">
        <HelpContent />
      </div>
    </section>
  );
}
