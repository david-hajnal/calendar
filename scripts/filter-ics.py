#!/usr/bin/env python3
"""Keep events starting in [execution date, execution date + 3 calendar months).

Setup:
    python3 -m venv .venv-ics
    .venv-ics/bin/pip install -r scripts/filter-ics-requirements.txt
Usage:
    .venv-ics/bin/python scripts/filter-ics.py calendar.ics
    .venv-ics/bin/python scripts/filter-ics.py calendar.ics --date 2026-10-20

Defaults to today in the calendar's X-WR-TIMEZONE (or the local timezone).
Recurring series are expanded into standalone events, including exceptions.
Writes calendar.filtered.ics by default; refuses to overwrite existing files.
"""

import argparse
from copy import deepcopy
from datetime import date, datetime, time
from pathlib import Path
from zoneinfo import ZoneInfo

from dateutil.relativedelta import relativedelta
from icalendar import Calendar
import recurring_ical_events


def filter_calendar(calendar, execution_date, timezone):
    end_date = execution_date + relativedelta(months=3)
    start = datetime.combine(execution_date, time.min, timezone)
    end = datetime.combine(end_date, time.min, timezone)
    output = deepcopy(calendar)
    # Preserve calendar metadata and timezone definitions, but no unrelated tasks.
    output.subcomponents = [
        component for component in output.subcomponents
        if component.name == "VTIMEZONE"
    ]
    for event in recurring_ical_events.of(calendar).between(start, end):
        event_start = event.decoded("DTSTART")
        if isinstance(event_start, datetime):
            if event_start.tzinfo is None:
                event_start = event_start.replace(tzinfo=timezone)
            keep = start <= event_start < end
        else:
            keep = execution_date <= event_start < end_date
        if not keep or str(event.get("STATUS", "")).upper() == "CANCELLED":
            continue
        # recurring_ical_events tags every event (recurring or not) with a
        # RECURRENCE-ID. Rewrite each UID to be unique so expanded occurrences
        # and exceptions import as standalone events without detached recurrences.
        recurrence_id = event.pop("RECURRENCE-ID", None)
        if recurrence_id is not None:
            event["UID"] = "{}-{}".format(
                event["UID"], recurrence_id.to_ical().decode("ascii")
            )
        output.add_component(event)
    return output


def main():
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("input", type=Path)
    parser.add_argument("--date", type=date.fromisoformat, help="Execution date: YYYY-MM-DD")
    parser.add_argument("--timezone", help="Override calendar timezone, e.g. Europe/Budapest")
    parser.add_argument("-o", "--output", type=Path)
    args = parser.parse_args()
    output = args.output or args.input.with_name(args.input.stem + ".filtered.ics")
    if output.resolve() == args.input.resolve():
        parser.error("output must differ from input")
    try:
        calendar = Calendar.from_ical(args.input.read_bytes())
        timezone_name = args.timezone or calendar.get("X-WR-TIMEZONE")
        timezone = (ZoneInfo(str(timezone_name)) if timezone_name
                    else datetime.now().astimezone().tzinfo)
        execution_date = args.date or datetime.now(timezone).date()
        filtered = filter_calendar(calendar, execution_date, timezone)
        with output.open("xb") as destination:
            destination.write(filtered.to_ical())
    except (OSError, ValueError, KeyError) as error:
        parser.exit(1, "Error: {}\n".format(error))
    count = len(filtered.walk("VEVENT"))
    print("Kept {} events from {} up to {} (exclusive): {}".format(
        count, execution_date, execution_date + relativedelta(months=3), output))


if __name__ == "__main__":
    main()
