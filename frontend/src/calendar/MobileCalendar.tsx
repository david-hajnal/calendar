import { useEffect, useRef } from "react";
import "./MobileCalendar.css";

// Temporary tracer content, removed when Slice 2 connects live projections.
export function SampleAgenda({ onReturn }: { onReturn(): void }) {
  const heading = useRef<HTMLHeadingElement>(null);
  useEffect(() => { heading.current?.focus(); }, []);
  return <section className="mobile-calendar" aria-labelledby="sample-agenda-heading">
    <button type="button" onClick={onReturn}>Return to your calendar</button>
    <h2 ref={heading} tabIndex={-1} id="sample-agenda-heading">Sample Agenda</h2>
    <p>Sample events only — this preview does not change your calendar.</p>
    <h3>Friday · 9 October 2026</h3>
    <ul className="mobile-calendar__events">
      <li><article><h4>Design review and next week’s priorities</h4><p>09:30–10:30</p><p>Work · Meeting room 2</p></article></li>
      <li><article><h4>Dinner with family</h4><p>18:00–19:30</p><p>Personal · Home</p></article></li>
    </ul>
    <h3>Saturday · 10 October 2026</h3>
    <ul className="mobile-calendar__events"><li><article><h4>Weekend away</h4><p>All day</p><p>Personal</p></article></li></ul>
  </section>;
}
