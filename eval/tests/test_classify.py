import unittest
from pathlib import Path

from b5.classify import anchor, closing
from b5.turns import turns
from wire import BLOCK, PUT, Wire


def one(w):
    (t,) = turns(w.events)
    return t


class Turns(unittest.TestCase):
    def test_a_round_is_a_usage_with_its_calls_and_text(self):
        w = Wire().start("Fix the race.")
        ids = w.round([("read", {"path": "a"}), ("cards_block", PUT)], usage={"inp": 900, "out": 40, "read": 100, "write": 7})
        w.results(ids)
        w.round(text="Done.", usage={"inp": 50, "out": 5})
        t = one(w.done())
        self.assertEqual((t.trigger, t.user_initiated, t.outcome, t.user_text), ("user", True, "done", "Fix the race."))
        self.assertEqual(len(t.rounds), 2)
        self.assertEqual([c.name for c in t.rounds[0].calls], ["read", "cards_block"])
        self.assertEqual((t.rounds[0].input_side, t.rounds[0].output), (1007, 40))
        self.assertEqual(t.rounds[1].text, "Done.")
        self.assertEqual([r.name for r in t.results()], ["read", "cards_block"])
        self.assertEqual([x[0] for x in t.timeline], ["user", "round", "result", "result", "round"])

    def test_a_call_in_either_wire_form_is_one_call_with_its_input(self):
        for form in ("whole", "streamed", "both"):
            with self.subTest(form=form):
                w = Wire(form).start()
                ids = w.round([("read", {"path": "a"}), ("cards_block", PUT)])
                w.results(ids)
                w.round([("cards_block", BLOCK)])
                t = one(w.done())
                self.assertEqual([[(c.name, c.input) for c in r.calls] for r in t.rounds],
                                 [[("read", {"path": "a"}), ("cards_block", PUT)], [("cards_block", BLOCK)]])
                self.assertEqual([r.name for r in t.results()], ["read", "cards_block"])

    def test_a_streamed_call_reset_mid_arguments_is_drawn_again_whole(self):
        w = Wire("streamed").start()
        w.stream("llm", "response_start").stream("llm", "tool_use_start", tool_name="cards_block", tool_id="t1")
        w.stream("llm", "tool_use_delta", tool_id="t1", delta='{"ops": [')
        w.stream("llm", "response_reset")
        w.stream("llm", "tool_use_start", tool_name="cards_block", tool_id="t1")
        w.stream("llm", "tool_use_delta", tool_id="t1", delta='{"ops": []}')
        w.usage()
        t = one(w.done())
        self.assertEqual([(c.name, c.input) for c in t.rounds[0].calls], [("cards_block", {"ops": []})])

    def test_a_reset_drops_what_the_round_drew(self):
        w = Wire().start()
        w.stream("llm", "response_start").stream("llm", "tool_use", tool_name="bash", tool_id="t1", input={})
        w.stream("llm", "response_reset")
        ids = w.round([("cards_block", BLOCK)], start=False)
        t = one(w.results(ids).done())
        self.assertEqual(len(t.rounds), 1)
        self.assertEqual(([c.name for c in t.rounds[0].calls], t.rounds[0].resets), (["cards_block"], 1))

    def test_rounds_are_counted_by_usage_even_with_no_response_start(self):
        w = Wire().start()
        w.results(w.round([("bash", {})], start=False))
        w.results(w.round([("cards_block", BLOCK)], start=False))
        w.round(text="Done.", start=False)
        t = one(w.done())
        self.assertEqual([len(r.calls) for r in t.rounds], [1, 1, 0])
        self.assertEqual([x[0] for x in t.timeline], ["user", "round", "result", "round", "result", "round"])

    def test_two_usages_with_nothing_between_are_two_rounds(self):
        w = Wire().start()
        w.stream("llm", "response_start").usage().usage()
        self.assertEqual(len(one(w.done()).rounds), 2)

    def test_who_started_a_turn(self):
        w = Wire()
        w.start(None, trigger="event_auto").round(text="Saw it.")
        w.done()
        w.start("and also", trigger="queued_auto").round(text="Sure.")
        w.done()
        w.start("Next.").round(text="Hi.")
        a, b, c = turns(w.done().events)
        self.assertEqual([x.user_initiated for x in (a, b, c)], [False, True, True])
        self.assertEqual(a.timeline[0][0], "round")

    def test_a_turn_with_no_idle_is_incomplete(self):
        w = Wire().start()
        w.round([("bash", {})])
        w.start("again").round(text="ok")
        a, b = turns(w.done().events)
        self.assertEqual((a.outcome, b.outcome), ("incomplete", "done"))

    def test_done_with_no_idle_is_no_complete_turn(self):
        # The stream ends after `done`, before the turn's `idle`, and the
        # response never got its usage: no round, nothing complete.
        w = Wire().start()
        w.stream("llm", "response_start").stream("llm", "text", text="Fixed.").stream("session", "done")
        t = one(w)
        self.assertEqual((len(t.rounds), t.outcome, t.t_end, closing(t).complete), (0, "incomplete", None, False))
        self.assertEqual((len(t.unmeasured), t.measured, t.saw_done), (1, False, True))

    def test_a_response_whose_usage_never_came_is_no_round(self):
        # Two measured rounds, then a final response that streamed text and
        # was cut before its usage: zero tokens it wasn't.
        w = Wire().start()
        w.results(w.round([("bash", {})]))
        w.results(w.round([("cards_block", BLOCK)]))
        w.stream("llm", "response_start").stream("llm", "text", text="Done.")
        t = one(w.done())
        self.assertEqual([x[0] for x in t.timeline], ["user", "round", "result", "round", "result"])
        self.assertFalse(closing(t).complete, "an unmeasured response could hide a closing round")
        self.assertEqual((len(t.rounds), [r.index for r in t.rounds], len(t.unmeasured)), (2, [0, 1], 1))
        self.assertEqual(t.unmeasured[0].text, "Done.")
        self.assertEqual((t.outcome, t.measured), ("done", False))

    def test_a_terminal_failure_is_no_round_and_no_measured_turn(self):
        w = Wire().start()
        w.stream("llm", "response_start").stream("llm", "text", text="Let me")
        w.stream("session", "error", message="overloaded").ev({"ev": "idle"})
        t = one(w)
        self.assertEqual((len(t.rounds), t.outcome), (0, "error"))
        self.assertEqual((len(t.unmeasured), t.measured), (1, False))

    def test_a_retry_that_fails_is_no_round(self):
        w = Wire().start()
        w.results(w.round([("bash", {})]))
        w.stream("llm", "response_start").stream("llm", "text", text="Half")
        w.stream("llm", "response_reset").stream("llm", "text", text="Half again")
        w.stream("session", "error", message="stream reset twice").ev({"ev": "idle"})
        t = one(w)
        self.assertEqual((len(t.rounds), t.outcome), (1, "error"))
        self.assertEqual(([r.resets for r in t.unmeasured], t.measured), ([1], False))

    def test_a_turn_with_its_idle_and_every_usage_is_measured(self):
        w = Wire().start()
        w.results(w.round([("bash", {})]))
        w.round(text="Done.")
        self.assertTrue(one(w.done()).measured)
        w = Wire().start()
        w.results(w.round([("bash", {})]))
        w.ev({"ev": "aborted", "context_saved": True}).ev({"ev": "idle"})
        self.assertEqual((one(w).outcome, one(w).measured), ("aborted", False))

    def test_steers_sit_where_the_engine_delivered_them(self):
        w = Wire().start()
        w.results(w.round([("bash", {})]))
        w.steer("also the docs")
        w.round(text="Docs too.")
        t = one(w.done())
        self.assertEqual(t.timeline[3][:3], ("user", "also the docs", True))
        self.assertEqual(t.steers, [("also the docs", True)])


class Classifier(unittest.TestCase):
    def test_blocks_off_k_is_1(self):
        w = Wire().start()
        w.results(w.round([("read", {})]))
        w.results(w.round([("bash", {})]))
        w.round(text="Fixed.")
        c = closing(one(w.done()))
        self.assertEqual((c.k, c.closing_rounds, c.closing_input, c.complete), (1, 0, 0, True))

    def test_a_call_alone_after_the_last_result_is_one_closing_round(self):
        w = Wire().start()
        w.results(w.round([("bash", {})]))
        w.results(w.round([("cards_block", BLOCK)], usage={"inp": 100, "read": 5000}))
        w.round(text="Fixed.", usage={"inp": 30, "read": 5100, "write": 20})
        c = closing(one(w.done()))
        self.assertEqual((c.k, c.closing_rounds), (2, 1))
        self.assertEqual((c.closing_input, c.closing_uncached, c.closing_cache_read, c.closing_cache_write), (5150, 30, 5100, 20))

    def test_a_call_riding_the_last_real_call_costs_no_round(self):
        w = Wire().start()
        w.results(w.round([("bash", {}), ("cards_block", BLOCK)]))
        w.round(text="Fixed.")
        self.assertEqual(closing(one(w.done())).closing_rounds, 0)

    def test_the_final_text_with_the_call_still_closes_once(self):
        # §2.8: k − 1, whichever response carried the final text.
        w = Wire().start()
        w.results(w.round([("bash", {})]))
        w.results(w.round([("cards_block", BLOCK)], text="All fixed: the race is gone."))
        w.round(text="")
        self.assertEqual(closing(one(w.done())).closing_rounds, 1)

    def test_two_closing_calls_are_two_closing_rounds(self):
        w = Wire().start()
        w.results(w.round([("bash", {})]))
        w.results(w.round([("cards_block", BLOCK)]))
        w.results(w.round([("cards_block", BLOCK)]))
        w.round(text="Fixed.")
        self.assertEqual(closing(one(w.done())).closing_rounds, 2)

    def test_a_block_only_response_before_another_tool_is_no_closing_round_but_is_counted(self):
        # Shady's trace (review #14): real tool, block-only, `bash true`, final text.
        w = Wire().start()
        w.results(w.round([("read", {})]))
        w.results(w.round([("cards_block", BLOCK)]))
        w.results(w.round([("bash", {"command": "true"}), ("cards_block", BLOCK)]))
        w.round(text="Fixed.")
        t = one(w.done())
        self.assertEqual(closing(t).closing_rounds, 0)
        self.assertEqual([r.block_only for r in t.rounds], [False, True, False, False], "a block call riding bash isn't block-only")

    def test_a_card_is_the_turns_work(self):
        w = Wire().start()
        w.results(w.round([("cards_block", PUT)]))
        w.results(w.round([("cards_ui", {"question": "Which?", "options": ["a", "b"]})]))
        w.round(text="Waiting for your answer.")
        self.assertEqual(closing(one(w.done())).closing_rounds, 0)

    def test_no_other_tool_anchors_at_the_users_message(self):
        w = Wire().start("Plan it.")
        w.results(w.round([("cards_block", PUT)]))
        w.round(text="Here's the plan.")
        c = closing(one(w.done()))
        self.assertEqual((c.k, c.closing_rounds), (2, 1))

    def test_a_response_after_a_steer_answers_the_user(self):
        w = Wire().start()
        w.results(w.round([("bash", {})]))
        w.results(w.round([("cards_block", BLOCK)]))
        w.steer("also bump the version")
        w.round(text="Bumped.")
        self.assertEqual(closing(one(w.done())).closing_rounds, 0)

    def test_blocks_off_a_steer_after_the_text_keeps_k_1(self):
        # The steer the turn's last drain took: the response after it is
        # the turn's one, so k stays 1 with blocks off (§2.8).
        w = Wire().start()
        w.results(w.round([("bash", {})]))
        w.round(text="Fixed.")
        w.steer("thanks, and the docs?")
        w.round(text="Docs are fine.")
        c = closing(one(w.done()))
        self.assertEqual((c.k, c.closing_rounds), (1, 0))

    def test_an_event_started_turn_with_nothing_to_anchor_takes_every_round(self):
        w = Wire().start(None, trigger="event_auto")
        w.results(w.round([("cards_block", BLOCK)]))
        w.round(text="Noted.")
        t = one(w.done())
        self.assertEqual((anchor(t), closing(t).closing_rounds), (-1, 1))

    def test_a_turn_cut_mid_close_is_counted_but_not_complete(self):
        # Cancelled after a closing call's round: its rounds are classified,
        # and the turn is left out of the complete ones the gate reads.
        w = Wire().start()
        w.results(w.round([("bash", {})]))
        w.results(w.round([("cards_block", BLOCK)]))
        w.round([("cards_block", BLOCK)])
        w.ev({"ev": "aborted", "context_saved": True}).ev({"ev": "idle"})
        c = closing(one(w))
        self.assertEqual((c.k, c.closing_rounds, c.complete), (2, 1, False))

    def test_a_turn_cut_after_a_result_has_no_rounds_to_close(self):
        w = Wire().start()
        w.results(w.round([("bash", {})]))
        w.ev({"ev": "aborted", "context_saved": True}).ev({"ev": "idle"})
        c = closing(one(w))
        self.assertEqual((c.k, c.closing_rounds, c.complete), (0, 0, False))


class Authority(unittest.TestCase):
    """§2.8 states what the classifier and the counter do (review #14 round
    2: the readings B5a made are ratified in the authority, not left in a
    module's docs)."""

    DOC = (Path(__file__).resolve().parents[2] / "docs" / "blocks.md").read_text()

    def test_2_8_states_the_latest_delivered_steer_and_the_tail_only_limit(self):
        self.assertIn("the later of the turn's last result from a tool other than `cards_block` and the latest user message "
                      "delivered in the turn", self.DOC)
        self.assertIn("A steer the engine queued past the turn's end starts its own turn, which is user-initiated", self.DOC)
        self.assertIn("**It is a tail metric.**", self.DOC)

    def test_2_8_states_the_two_framings(self):
        self.assertIn("**The result's framing** (the pair's allowance less the call's) is input overhead", self.DOC)
        self.assertIn("never attributed to block-call output", self.DOC)


if __name__ == "__main__":
    unittest.main()
