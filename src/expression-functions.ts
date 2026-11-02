/** Built-in expression functions offered by the picker; `|` marks the caret position. */

export const EXPRESSION_FUNCTIONS: ReadonlyArray<{ sig: string; desc: string; template: string }> = [
  // String
  { sig: "upper(string)",               desc: "Uppercase a string",                         template: "{{upper(|)}}" },
  { sig: "lower(string)",               desc: "Lowercase a string",                         template: "{{lower(|)}}" },
  { sig: "trim(string)",                desc: "Strip leading and trailing whitespace",        template: "{{trim(|)}}" },
  { sig: "trim_start(string)",          desc: "Strip leading whitespace",                    template: "{{trim_start(|)}}" },
  { sig: "trim_end(string)",            desc: "Strip trailing whitespace",                   template: "{{trim_end(|)}}" },
  { sig: "len(string|array)",           desc: "Character or element count",                  template: "{{len(|)}}" },
  { sig: "slice(string, start, end)",   desc: "Substring by character index (0-based)",      template: "{{slice(|, 0, 10)}}" },
  { sig: "replace(string, from, to)",   desc: "Replace first occurrence of a substring",     template: "{{replace(|, \"old\", \"new\")}}" },
  { sig: "replace_all(string, from, to)", desc: "Replace all occurrences of a substring",   template: "{{replace_all(|, \"old\", \"new\")}}" },
  { sig: "contains(string, substr)",    desc: "True if string contains substr",              template: "{{contains(|, \"text\")}}" },
  { sig: "starts_with(string, prefix)", desc: "True if string starts with prefix",           template: "{{starts_with(|, \"prefix\")}}" },
  { sig: "ends_with(string, suffix)",   desc: "True if string ends with suffix",             template: "{{ends_with(|, \"suffix\")}}" },
  { sig: "split(string, delimiter)",    desc: "Split into an array",                         template: "{{split(|, \",\")}}" },
  { sig: "join(array, delimiter)",      desc: "Join array elements into a string",           template: "{{join(|, \",\")}}" },
  { sig: "pad_start(string, len, char)", desc: "Left-pad string to length",                 template: "{{pad_start(|, 10, \"0\")}}" },
  { sig: "pad_end(string, len, char)",  desc: "Right-pad string to length",                  template: "{{pad_end(|, 10, \" \")}}" },
  // Number
  { sig: "round(number, decimals)",     desc: "Round to N decimal places",                  template: "{{round(|, 2)}}" },
  { sig: "floor(number)",               desc: "Round down to nearest integer",               template: "{{floor(|)}}" },
  { sig: "ceil(number)",                desc: "Round up to nearest integer",                 template: "{{ceil(|)}}" },
  { sig: "abs(number)",                 desc: "Absolute value",                              template: "{{abs(|)}}" },
  { sig: "min(a, b)",                   desc: "Smaller of two numbers",                      template: "{{min(|, 0)}}" },
  { sig: "max(a, b)",                   desc: "Larger of two numbers",                       template: "{{max(|, 0)}}" },
  { sig: "to_int(string)",              desc: "Parse string to integer",                     template: "{{to_int(|)}}" },
  { sig: "to_float(string)",            desc: "Parse string to decimal number",              template: "{{to_float(|)}}" },
  // Date
  { sig: "now()",                       desc: "Current UTC time as ISO 8601 string",         template: "{{now()}}" },
  { sig: "format_date(timestamp, fmt)", desc: "Format ISO timestamp — tokens: YYYY MM DD HH mm ss", template: "{{format_date(|, \"YYYY-MM-DD\")}}" },
  { sig: "parse_date(string)",          desc: "Parse a date string to ISO 8601",             template: "{{parse_date(|)}}" },
  { sig: "add_days(timestamp, n)",      desc: "Add N days to a timestamp",                   template: "{{add_days(|, 1)}}" },
  { sig: "add_hours(timestamp, n)",     desc: "Add N hours to a timestamp",                  template: "{{add_hours(|, 1)}}" },
  { sig: "add_minutes(timestamp, n)",   desc: "Add N minutes to a timestamp",                template: "{{add_minutes(|, 30)}}" },
  { sig: "date_diff(ts1, ts2, unit)",   desc: "Difference between timestamps — unit: days hours minutes seconds", template: "{{date_diff(|, \"\", \"days\")}}" },
  // Array / Object
  { sig: "first(array)",                desc: "First element of an array",                   template: "{{first(|)}}" },
  { sig: "last(array)",                 desc: "Last element of an array",                    template: "{{last(|)}}" },
  { sig: "nth(array, index)",           desc: "Element at 0-based index",                    template: "{{nth(|, 0)}}" },
  { sig: "keys(object)",                desc: "Array of object keys",                        template: "{{keys(|)}}" },
  { sig: "values(object)",              desc: "Array of object values",                      template: "{{values(|)}}" },
  // Conditional
  { sig: "if(condition, then, else)",   desc: "Return then if condition is truthy, else otherwise", template: "{{if(|, \"\", \"\")}}" },
];
