import type { ChangeEvent } from "react";
import type { RepeatFrequency, RepeatSettings } from "./recurrenceRule";
import "./RepeatForm.css";

export interface RepeatFormProps {
  value: RepeatSettings;
  onChange: (settings: RepeatSettings) => void;
}

const FREQUENCY_OPTIONS: { value: RepeatFrequency; label: string }[] = [
  { value: "none", label: "Never" },
  { value: "daily", label: "Daily" },
  { value: "weekly", label: "Weekly" },
  { value: "monthly", label: "Monthly" },
  { value: "yearly", label: "Yearly" },
];

export function RepeatForm({ value, onChange }: RepeatFormProps) {
  const isRepeating = value.frequency !== "none";

  function handleFrequencyChange(event: ChangeEvent<HTMLSelectElement>) {
    const frequency = event.target.value as RepeatFrequency;
    onChange({ ...value, frequency });
  }

  function handleIntervalChange(event: ChangeEvent<HTMLInputElement>) {
    const interval = Math.max(1, Number(event.target.value) || 1);
    onChange({ ...value, interval });
  }

  function handleEndKindChange(event: ChangeEvent<HTMLSelectElement>) {
    const kind = event.target.value as RepeatSettings["end"]["kind"];
    if (kind === "never") {
      onChange({ ...value, end: { kind: "never" } });
    } else if (kind === "after") {
      onChange({ ...value, end: { kind: "after", count: 1 } });
    } else {
      onChange({ ...value, end: { kind: "on", date: "" } });
    }
  }

  function handleEndCountChange(event: ChangeEvent<HTMLInputElement>) {
    const count = Math.max(1, Number(event.target.value) || 1);
    onChange({ ...value, end: { kind: "after", count } });
  }

  function handleEndDateChange(event: ChangeEvent<HTMLInputElement>) {
    onChange({ ...value, end: { kind: "on", date: event.target.value } });
  }

  return (
    <fieldset className="repeat-form">
      <legend className="repeat-form__legend">Repeat</legend>

      <div className="repeat-form__field">
        <label className="repeat-form__label" htmlFor="repeat-frequency">
          Frequency
        </label>
        <select
          id="repeat-frequency"
          className="repeat-form__select"
          value={value.frequency}
          onChange={handleFrequencyChange}
        >
          {FREQUENCY_OPTIONS.map((option) => (
            <option key={option.value} value={option.value}>
              {option.label}
            </option>
          ))}
        </select>
      </div>

      {isRepeating && (
        <>
          <div className="repeat-form__field">
            <label className="repeat-form__label" htmlFor="repeat-interval">
              Every
            </label>
            <input
              id="repeat-interval"
              className="repeat-form__input"
              type="number"
              min={1}
              value={value.interval}
              onChange={handleIntervalChange}
            />
          </div>

          <div className="repeat-form__field">
            <label className="repeat-form__label" htmlFor="repeat-end">
              End
            </label>
            <select
              id="repeat-end"
              className="repeat-form__select"
              value={value.end.kind}
              onChange={handleEndKindChange}
            >
              <option value="never">Never</option>
              <option value="after">After N occurrences</option>
              <option value="on">On date</option>
            </select>
          </div>

          {value.end.kind === "after" && (
            <div className="repeat-form__field">
              <label className="repeat-form__label" htmlFor="repeat-end-count">
                Occurrences
              </label>
              <input
                id="repeat-end-count"
                className="repeat-form__input"
                type="number"
                min={1}
                value={value.end.count}
                onChange={handleEndCountChange}
              />
            </div>
          )}

          {value.end.kind === "on" && (
            <div className="repeat-form__field">
              <label className="repeat-form__label" htmlFor="repeat-end-date">
                Date
              </label>
              <input
                id="repeat-end-date"
                className="repeat-form__input"
                type="date"
                value={value.end.date}
                onChange={handleEndDateChange}
              />
            </div>
          )}
        </>
      )}
    </fieldset>
  );
}
