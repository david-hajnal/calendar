import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { RepeatForm } from "./RepeatForm";
import type { RepeatSettings } from "./recurrenceRule";

function baseSettings(overrides: Partial<RepeatSettings> = {}): RepeatSettings {
  return { frequency: "none", interval: 1, end: { kind: "never" }, ...overrides };
}

function renderForm(settings: RepeatSettings, onChange = vi.fn()) {
  render(<RepeatForm value={settings} onChange={onChange} />);
  return onChange;
}

afterEach(() => { cleanup(); });

describe("RepeatForm", () => {
  describe("frequency select", () => {
    it("renders a frequency select limited to Never/Daily/Weekly/Monthly/Yearly", () => {
      renderForm(baseSettings());
      const select = screen.getByRole("combobox", { name: "Frequency" });
      const options = Array.from(select.querySelectorAll("option"));
      expect(options.map((o) => o.textContent)).toEqual(["Never", "Daily", "Weekly", "Monthly", "Yearly"]);
    });

    it("does not render weekday or BYDAY options", () => {
      renderForm(baseSettings());
      const select = screen.getByRole("combobox", { name: "Frequency" });
      const optionValues = Array.from(select.querySelectorAll("option")).map((o) => o.value);
      expect(optionValues).not.toContain("byday");
      expect(optionValues).not.toContain("weekday");
    });

    it.each([
      ["none", "Never"],
      ["daily", "Daily"],
      ["weekly", "Weekly"],
      ["monthly", "Monthly"],
      ["yearly", "Yearly"],
    ] as const)("selecting %s frequency emits the expected RepeatSettings", (frequency, label) => {
      const onChange = vi.fn();
      renderForm(baseSettings({ frequency: "none" }), onChange);
      const select = screen.getByRole("combobox", { name: "Frequency" });
      expect((screen.getByRole("option", { name: label }) as HTMLOptionElement).value).toBe(frequency);
      fireEvent.change(select, { target: { value: frequency } });
      expect(onChange).toHaveBeenCalledWith(
        expect.objectContaining({ frequency }),
      );
    });

    it("reflects the current frequency value", () => {
      renderForm(baseSettings({ frequency: "weekly" }));
      const select = screen.getByRole("combobox", { name: "Frequency" });
      expect(select).toHaveValue("weekly");
    });
  });

  describe("interval control", () => {
    it("is hidden when frequency is none", () => {
      renderForm(baseSettings({ frequency: "none" }));
      expect(screen.queryByRole("spinbutton", { name: "Every" })).not.toBeInTheDocument();
    });

    it("is visible when frequency is not none", () => {
      renderForm(baseSettings({ frequency: "daily" }));
      expect(screen.getByRole("spinbutton", { name: "Every" })).toBeInTheDocument();
    });

    it("reflects the current interval value", () => {
      renderForm(baseSettings({ frequency: "weekly", interval: 3 }));
      const input = screen.getByRole("spinbutton", { name: "Every" });
      expect(input).toHaveValue(3);
    });

    it("emits the updated interval on change", () => {
      const onChange = vi.fn();
      renderForm(baseSettings({ frequency: "daily", interval: 1 }), onChange);
      const input = screen.getByRole("spinbutton", { name: "Every" });
      fireEvent.change(input, { target: { value: "4" } });
      expect(onChange).toHaveBeenCalledWith(
        expect.objectContaining({ interval: 4 }),
      );
    });

    it("clamps interval to a minimum of 1", () => {
      const onChange = vi.fn();
      renderForm(baseSettings({ frequency: "daily", interval: 1 }), onChange);
      const input = screen.getByRole("spinbutton", { name: "Every" });
      fireEvent.change(input, { target: { value: "0" } });
      expect(onChange).toHaveBeenCalledWith(
        expect.objectContaining({ interval: 1 }),
      );
    });
  });

  describe("end controls", () => {
    it("are hidden when frequency is none", () => {
      renderForm(baseSettings({ frequency: "none" }));
      expect(screen.queryByRole("combobox", { name: "End" })).not.toBeInTheDocument();
    });

    it("are visible when frequency is not none", () => {
      renderForm(baseSettings({ frequency: "daily" }));
      expect(screen.getByRole("combobox", { name: "End" })).toBeInTheDocument();
    });

    it("renders end options: Never, After N occurrences, On date", () => {
      renderForm(baseSettings({ frequency: "daily" }));
      const select = screen.getByRole("combobox", { name: "End" });
      const options = Array.from(select.querySelectorAll("option"));
      expect(options.map((o) => o.textContent)).toEqual(["Never", "After N occurrences", "On date"]);
    });

    it("reflects the current end kind", () => {
      renderForm(baseSettings({ frequency: "daily", end: { kind: "after", count: 5 } }));
      const select = screen.getByRole("combobox", { name: "End" });
      expect(select).toHaveValue("after");
    });

    it("emits end kind never on change", () => {
      const onChange = vi.fn();
      renderForm(baseSettings({ frequency: "daily", end: { kind: "after", count: 5 } }), onChange);
      const select = screen.getByRole("combobox", { name: "End" });
      fireEvent.change(select, { target: { value: "never" } });
      expect(onChange).toHaveBeenCalledWith(
        expect.objectContaining({ end: { kind: "never" } }),
      );
    });

    it("emits end kind after with default count on change", () => {
      const onChange = vi.fn();
      renderForm(baseSettings({ frequency: "daily", end: { kind: "never" } }), onChange);
      const select = screen.getByRole("combobox", { name: "End" });
      fireEvent.change(select, { target: { value: "after" } });
      expect(onChange).toHaveBeenCalledWith(
        expect.objectContaining({ end: { kind: "after", count: 1 } }),
      );
    });

    it("emits end kind on with empty date on change", () => {
      const onChange = vi.fn();
      renderForm(baseSettings({ frequency: "daily", end: { kind: "never" } }), onChange);
      const select = screen.getByRole("combobox", { name: "End" });
      fireEvent.change(select, { target: { value: "on" } });
      expect(onChange).toHaveBeenCalledWith(
        expect.objectContaining({ end: { kind: "on", date: "" } }),
      );
    });

    it("shows count input when end kind is after", () => {
      renderForm(baseSettings({ frequency: "daily", end: { kind: "after", count: 5 } }));
      expect(screen.getByRole("spinbutton", { name: "Occurrences" })).toBeInTheDocument();
    });

    it("shows date input when end kind is on", () => {
      renderForm(baseSettings({ frequency: "daily", end: { kind: "on", date: "2026-12-25" } }));
      expect(screen.getByLabelText("Date")).toBeInTheDocument();
    });

    it("hides count and date inputs when end kind is never", () => {
      renderForm(baseSettings({ frequency: "daily", end: { kind: "never" } }));
      expect(screen.queryByRole("spinbutton", { name: "Occurrences" })).not.toBeInTheDocument();
      expect(screen.queryByLabelText("Date")).not.toBeInTheDocument();
    });

    it("emits updated count on change", () => {
      const onChange = vi.fn();
      renderForm(baseSettings({ frequency: "daily", end: { kind: "after", count: 5 } }), onChange);
      const input = screen.getByRole("spinbutton", { name: "Occurrences" });
      fireEvent.change(input, { target: { value: "10" } });
      expect(onChange).toHaveBeenCalledWith(
        expect.objectContaining({ end: { kind: "after", count: 10 } }),
      );
    });

    it("emits updated date on change", () => {
      const onChange = vi.fn();
      renderForm(baseSettings({ frequency: "daily", end: { kind: "on", date: "2026-12-25" } }), onChange);
      const input = screen.getByLabelText("Date");
      fireEvent.change(input, { target: { value: "2027-01-01" } });
      expect(onChange).toHaveBeenCalledWith(
        expect.objectContaining({ end: { kind: "on", date: "2027-01-01" } }),
      );
    });
  });

  describe("controlled behavior", () => {
    it("reflects a full RepeatSettings value", () => {
      const settings: RepeatSettings = {
        frequency: "monthly",
        interval: 2,
        end: { kind: "after", count: 12 },
      };
      renderForm(settings);

      expect(screen.getByRole("combobox", { name: "Frequency" })).toHaveValue("monthly");
      expect(screen.getByRole("spinbutton", { name: "Every" })).toHaveValue(2);
      expect(screen.getByRole("combobox", { name: "End" })).toHaveValue("after");
      expect(screen.getByRole("spinbutton", { name: "Occurrences" })).toHaveValue(12);
    });

    it("emits a complete RepeatSettings object on frequency change", () => {
      const onChange = vi.fn();
      const settings: RepeatSettings = {
        frequency: "none",
        interval: 1,
        end: { kind: "never" },
      };
      renderForm(settings, onChange);
      const select = screen.getByRole("combobox", { name: "Frequency" });
      fireEvent.change(select, { target: { value: "yearly" } });
      expect(onChange).toHaveBeenCalledWith({
        frequency: "yearly",
        interval: 1,
        end: { kind: "never" },
      });
    });
  });
});
