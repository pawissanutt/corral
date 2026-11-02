### Visual implementation review (v2)

Everything in the implementation contract applies, plus inspection of the actual UI evidence.

- Open and look at the supplied screenshots and interaction evidence. Never claim the UI was
  seen when it was not.
- Check the relevant viewport and state coverage, keyboard access and visual fidelity against
  the reference or stated intent.
- When needed visual evidence is absent, request it (`changes`) rather than approving.
- If evidence exists but your image tool cannot deliver it or your model cannot see images,
  do not claim visual inspection or submit a verdict. Ask the PM with `corral_ask` (kind
  `technical_choice`) to replace this attempt using `corral review-run <task> --reviewer
  <image-capable-profile>`. Include the screenshot artifact paths, exact tool error, and any
  findings so far. The PM selects a configured independent image-capable profile and starts
  that command; replacement supersedes this attempt and closes its pane. Do not wait to
  complete this original attempt: the replacement reviewer receives the candidate and visual
  contract, inspects the evidence, and records its own verdict. If no eligible image-capable
  profile is available, the PM must resolve that configuration limitation.
