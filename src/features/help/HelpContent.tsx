import { sections } from "./content";

export function HelpContent() {
  return (
    <>
      {sections.map((entry) => (
        <section key={entry.id} id={`help-${entry.id}`}>
          <h3 className="mt-6 mb-2 text-section font-semibold text-ink-strong">
            {entry.title}
          </h3>
          {entry.body}
        </section>
      ))}
    </>
  );
}
