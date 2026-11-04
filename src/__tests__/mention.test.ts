import { describe, expect, it } from "vitest";
import { isMentionStart, mentionQuery } from "../mention";

describe("isMentionStart", () => {
  it("accepts_an_at_at_the_start_of_the_field_or_after_whitespace", () => {
    expect(isMentionStart("@", 1)).toBe(true);
    expect(isMentionStart("Hello @", 7)).toBe(true);
    expect(isMentionStart("line one\n@", 10)).toBe(true);
  });

  it("rejects_an_at_after_a_non_space_character", () => {
    expect(isMentionStart("name@", 5)).toBe(false);
    expect(isMentionStart("<@", 2)).toBe(false);
    expect(isMentionStart("{{@", 3)).toBe(false);
  });

  it("rejects_a_caret_that_is_not_right_after_an_at", () => {
    expect(isMentionStart("@a", 2)).toBe(false);
    expect(isMentionStart("", 0)).toBe(false);
  });
});

describe("mentionQuery", () => {
  it("returns_the_text_between_the_at_and_the_caret", () => {
    expect(mentionQuery("Hi @summ", 3, 8)).toBe("summ");
    expect(mentionQuery("Hi @", 3, 4)).toBe("");
  });

  it("ends_when_the_at_is_gone_the_caret_moves_before_it_or_the_text_breaks", () => {
    expect(mentionQuery("Hi summ", 3, 7)).toBeNull();
    expect(mentionQuery("Hi @summ", 3, 3)).toBeNull();
    expect(mentionQuery("Hi @ summ", 3, 9)).toBeNull();
    expect(mentionQuery("Hi @a\nb", 3, 7)).toBeNull();
  });

  it("keeps_spaces_inside_the_query_for_node_names", () => {
    expect(mentionQuery("@Newest It", 0, 10)).toBe("Newest It");
  });
});
