# Prior art: bibliography

Sources behind [`prior-art.md`](prior-art.md). Entries marked **[unverified]**
carry a figure that could not be checked against a primary source; do not
restate one without the marker.

## Loop patterns

- Yao, Zhao, Yu, Du, Shafran, Narasimhan, Cao. *ReAct: Synergizing Reasoning and Acting in Language Models.* ICLR 2023. https://arxiv.org/abs/2210.03629
- Shinn, Cassano, Berman, Gopinath, Narasimhan, Yao. *Reflexion: Language Agents with Verbal Reinforcement Learning.* NeurIPS 2023. https://arxiv.org/abs/2303.11366
- Madaan et al. *Self-Refine: Iterative Refinement with Self-Feedback.* NeurIPS 2023. https://arxiv.org/abs/2303.17651
- Wang, Wei, Schuurmans, Le, Chi, Narang, Chowdhery, Zhou. *Self-Consistency Improves Chain of Thought Reasoning in Language Models.* ICLR 2023. https://arxiv.org/abs/2203.11171
- Yao, Yu, Zhao, Shafran, Griffiths, Cao, Narasimhan. *Tree of Thoughts.* NeurIPS 2023. https://arxiv.org/abs/2305.10601
- Besta et al. *Graph of Thoughts.* AAAI 2024. https://arxiv.org/abs/2308.09687
- Zhou, Yan, Shlapentokh-Rothman, Wang, Wang. *Language Agent Tree Search Unifies Reasoning, Acting and Planning.* ICML 2024. https://arxiv.org/abs/2310.04406
- Wang et al. *Plan-and-Solve Prompting.* ACL 2023. https://arxiv.org/abs/2305.04091
- Zhou et al. *Least-to-Most Prompting Enables Complex Reasoning.* ICLR 2023. https://arxiv.org/abs/2205.10625
- Sharma, Chopra. *The Sequential Edge: Inverse-Entropy Voting Beats Parallel Self-Consistency at Matched Compute.* 2025 preprint. https://arxiv.org/abs/2511.02309

## Verification and test-time compute

- Cobbe et al. *Training Verifiers to Solve Math Word Problems.* 2021. https://arxiv.org/abs/2110.14168 — **[unverified]** the widely quoted "equivalent to a 30x larger model" framing is not in the abstract.
- Lightman et al. *Let's Verify Step by Step.* ICLR 2024. https://arxiv.org/abs/2305.20050
- Huang, Chen, Mishra, Zheng, Yu, Song, Zhou. *Large Language Models Cannot Self-Correct Reasoning Yet.* ICLR 2024. https://arxiv.org/abs/2310.01798
- Valmeekam, Marquez, Kambhampati. *Can Large Language Models Really Improve by Self-critiquing Their Own Plans?* 2023. https://arxiv.org/abs/2310.08118
- Stechly, Marquez, Kambhampati. *GPT-4 Doesn't Know It's Wrong.* 2023. https://arxiv.org/abs/2310.12397
- Kamoi, Zhang, Zhang, Han, Zhang. *When Can LLMs Actually Correct Their Own Mistakes? A Critical Survey of Self-Correction of LLMs.* TACL 2024. https://arxiv.org/abs/2406.01297
- Gou et al. *CRITIC: Large Language Models Can Self-Correct with Tool-Interactive Critiquing.* ICLR 2024. https://arxiv.org/abs/2305.11738
- Song, Zhang, Eisenach, Kakade, Foster, Ghai. *Mind the Gap: Examining the Self-Improvement Capabilities of Large Language Models.* ICLR 2025. https://arxiv.org/abs/2412.02674
- Snell, Lee, Xu, Kumar. *Scaling LLM Test-Time Compute Optimally.* 2024. https://arxiv.org/abs/2408.03314
- Brown et al. *Large Language Monkeys: Scaling Inference Compute with Repeated Sampling.* 2024. https://arxiv.org/abs/2407.21787
- Gao, Schulman, Hilton. *Scaling Laws for Reward Model Overoptimization.* ICML 2023. https://arxiv.org/abs/2210.10760
- Chen et al. *Are More LLM Calls All You Need? Towards Scaling Laws of Compound Inference Systems.* 2024. https://arxiv.org/abs/2403.02419
- Zhang et al. *Generative Verifiers: Reward Modeling as Next-Token Prediction.* ICLR 2025. https://arxiv.org/abs/2408.15240
- Zheng et al. *Judging LLM-as-a-Judge with MT-Bench and Chatbot Arena.* NeurIPS 2023. https://arxiv.org/abs/2306.05685
- Khan et al. *Debating with More Persuasive LLMs Leads to More Truthful Answers.* ICML 2024. https://arxiv.org/abs/2402.06782
- Irving, Christiano, Amodei. *AI Safety via Debate.* 2018. https://arxiv.org/abs/1805.00899
- Nezhurina, Cipolina-Kun, Cherti, Jitsev. *Alice in Wonderland.* 2024. https://arxiv.org/abs/2406.02061

## Search, evolution, and symbolic verification

- Romera-Paredes et al. *Mathematical discoveries from program search with large language models* (FunSearch). Nature 625:468–475, 2024. https://doi.org/10.1038/s41586-023-06924-6 — **[unverified]** the exact bin-packing improvement percentage.
- Google DeepMind. *AlphaEvolve.* 2025 whitepaper. Related: https://arxiv.org/abs/2506.13242 — **[unverified]** the quoted annual dollar savings come from press coverage, not the primary source.
- Trinh, Wu, Le, He, Luong. *Solving olympiad geometry without human demonstrations.* Nature 625, 2024. https://doi.org/10.1038/s41586-023-06747-5
- Hubert, Mehta, Sartran et al. *Olympiad-level formal mathematical reasoning with reinforcement learning* (AlphaProof). Nature 651:607–613, 2025. https://doi.org/10.1038/s41586-025-09833-y
- Li et al. *Competition-Level Code Generation with AlphaCode.* Science 2022. https://arxiv.org/abs/2203.07814
- Wang et al. *Voyager: An Open-Ended Embodied Agent with Large Language Models.* 2023. https://arxiv.org/abs/2305.16291
- Zelikman, Wu, Mu, Goodman. *STaR: Bootstrapping Reasoning With Reasoning.* NeurIPS 2022. https://arxiv.org/abs/2203.14465
- Zhao et al. *Absolute Zero: Reinforced Self-play Reasoning with Zero Data.* 2025. https://arxiv.org/abs/2505.03335
- Bolan, Breitner, Brox, Carlini, Carneiro, Tao et al. *The Equational Theories Project.* 2025. https://arxiv.org/abs/2512.07087 — the paper reports the numbers cited; it does **not** state a refutation-before-proof policy, and `prior-art.md` presents that ordering as an inference.

## Agentic software engineering

- Jimenez, Yang, Wettig, Yao, Pei, Press, Narasimhan. *SWE-bench.* ICLR 2024. https://arxiv.org/abs/2310.06770
- Yang, Jimenez, Wettig, Lieret, Yao, Narasimhan, Press. *SWE-agent: Agent-Computer Interfaces Enable Automated Software Engineering.* NeurIPS 2024. https://arxiv.org/abs/2405.15793 — **[unverified]** the ablation table digits come from the HTML rendering rather than the PDF table; internally consistent with secondary summaries.
- Wang et al. *OpenHands: An Open Platform for AI Software Developers as Generalist Agents.* 2024. https://arxiv.org/abs/2407.16741
- Liu et al. *AgentBench: Evaluating LLMs as Agents.* ICLR 2024. https://arxiv.org/abs/2308.03688 — **[unverified]** the commercial-versus-open-source gap is stated qualitatively; no figure.
- Liang, Garg, Zilouchian Moghaddam. *The SWE-Bench Illusion.* ICSE-SEIP 2026. https://arxiv.org/abs/2506.12286
- *SWE-bench+: Enhanced Coding Benchmark for LLMs.* 2024. https://arxiv.org/abs/2410.06992
- OpenAI Preparedness. *Introducing SWE-bench Verified.* Aug 2024. https://openai.com/index/introducing-swe-bench-verified/ — a blog announcement and dataset, **not** a paper.
- *Inside the Scaffold.* 2026. https://arxiv.org/abs/2604.03515 — the 13-scaffold survey behind the converged/unsettled split.

## Memory and context

- Liu, Lin, Hewitt, Paranjape, Bevilacqua, Petroni, Liang. *Lost in the Middle.* TACL 2024. https://arxiv.org/abs/2307.03172 — **[unverified]** precise drop percentages are in the tables, not the abstract.
- Hsieh et al. *RULER: What's the Real Context Size of Your Long-Context Language Models?* COLM 2024. https://arxiv.org/abs/2404.06654
- Modarressi et al. *NoLiMa: Long-Context Evaluation Beyond Literal Matching.* ICML 2025. https://arxiv.org/abs/2502.05167
- Levy, Jacoby, Goldberg. *Same Task, More Tokens.* ACL 2024. https://arxiv.org/abs/2402.14848
- Hong, Troynikov, Huber. *Context Rot.* Chroma technical report, 2025. https://www.trychroma.com/research/context-rot — industry report, not peer reviewed.
- Packer et al. *MemGPT: Towards LLMs as Operating Systems.* 2023. https://arxiv.org/abs/2310.08560 — **[unverified]** specific benchmark scores.
- Fountas et al. *Human-inspired Episodic Memory for Infinite Context LLMs.* ICLR 2025. https://arxiv.org/abs/2407.09450
- Lewis et al. *Retrieval-Augmented Generation for Knowledge-Intensive NLP Tasks.* NeurIPS 2020. https://arxiv.org/abs/2005.11401
- Min, Wu, Darbari, Chen, Hong. *Toward Reliable Context Compression for Long-Horizon Agents.* 2026. https://arxiv.org/abs/2608.06503 — the only directly verified compaction study.

There is **no rigorous head-to-head study of episodic versus semantic memory
formats in LLM agents**, and no compaction paper playing the role RULER plays
for raw long context. Claims of that shape should be treated as unverified.

## Multi-agent

- Du, Li, Torralba, Tenenbaum, Mordatch. *Improving Factuality and Reasoning in Language Models through Multiagent Debate.* 2023. https://arxiv.org/abs/2305.14325 — **[unverified]** the quoted 5–10 point gain is secondary-sourced, and the paper lacks an equal-compute single-agent baseline.
- Smit, Duckworth, Grinsztajn, Barrett, Pretorius. *Should we be going MAD?* ICML 2024. https://arxiv.org/abs/2311.17371
- Cemri et al. *Why Do Multi-Agent LLM Systems Fail?* (the MAST taxonomy). 2025. https://arxiv.org/abs/2503.13657 — **[unverified]** the +9.4%/+15.6% intervention figures are secondary-sourced.

No peer-reviewed source with a clean single-versus-multi-agent token-cost figure
was found. Company engineering posts reporting multiples should be cited as
such.

## Reliability and economics

- Kwa et al. (METR). *Measuring AI Ability to Complete Long Tasks.* NeurIPS 2025. https://arxiv.org/abs/2503.14499 — **[unverified]** the exact per-step failure functional form is in the body, not the abstract.
- Sinha, Arun, Goel, Staab, Geiping. *The Illusion of Diminishing Returns: Measuring Long Horizon Execution in LLMs.* 2025. https://arxiv.org/abs/2509.09677
- Chen, Zaharia, Zou. *FrugalGPT.* 2023. https://arxiv.org/abs/2305.05176 — **[unverified]** the 98%/4% figures are secondary-sourced.
- Kapoor, Stroebl, Siegel, Nadgir, Narayanan. *AI Agents That Matter.* 2024. https://arxiv.org/abs/2407.01502

## Practice: loop engineering and harness design

- Osmani. *Loop Engineering.* 2026. https://addyosmani.com/blog/loop-engineering/
- cobusgreyling. *loop-engineering* pattern library. https://github.com/cobusgreyling/loop-engineering
- Willison. *Designing agentic loops.* 2025. https://simonwillison.net/2025/Sep/30/designing-agentic-loops/
- Anthropic. *Building effective agents.* https://www.anthropic.com/engineering/building-effective-agents
- Anthropic. *Effective context engineering for AI agents.* https://www.anthropic.com/engineering/effective-context-engineering-for-ai-agents
- Anthropic. *Writing effective tools for agents.* https://www.anthropic.com/engineering/writing-tools-for-agents
- Anthropic. *Effective harnesses for long-running agents.* https://www.anthropic.com/engineering/effective-harnesses-for-long-running-agents
- Anthropic. *April 23 postmortem.* 2026. https://www.anthropic.com/engineering/april-23-postmortem — the three product-layer regressions behind C-17.
- Cognition. *Don't Build Multi-Agents.* https://cognition.com/blog/dont-build-multi-agents
- Manus. *Context Engineering for AI Agents.* https://manus.im/blog/Context-Engineering-for-AI-Agents-Lessons-from-Building-Manus
- Lulla et al. *Loop Engineering: Building Blocks, Adoption, and Impact.* 2026. https://arxiv.org/abs/2608.21884 — 36,710 repositories mined, 217 real autonomous loops confirmed.

One source named in the original research brief — a practitioner discussion
thread on a forum whose domain is blocked to automated fetching — could not be
retrieved. Its criticism is not represented in `prior-art.md`.
