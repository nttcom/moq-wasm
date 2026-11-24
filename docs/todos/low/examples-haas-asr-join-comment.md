# [XS] Correct the comment that says the transcriber joins segments

Next: rewrite the comment at `examples/python/haas-asr-server/haas_asr_server/model.py:25`.

## Goal

The comment says "The bot joins the segments that arrive while this is busy".
The bot transcribes each segment on its own, in order (`moq_meeting_transcriber/stt.py`, `UtteranceSTTService.run_stt`), so the reason given for `transcribe_longform` is wrong.
