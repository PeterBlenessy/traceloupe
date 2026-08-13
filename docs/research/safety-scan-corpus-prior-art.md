# What public data already exists for the categories called "unlabelled"

**Status: desk research, 2026-08-13. Nothing here has been downloaded, licensed,
or measured.** It exists so the corpus effort (journey §8) starts from what is
already available rather than generating over the top of it. Every claim below
is marked with how strongly it is held; the licence questions in particular are
**unresolved** and block use, not merely inform it.

## Why this note exists

Journey §8 says of coercive-control, grooming and relationship-harassment: *"The
only route is to **generate** labelled conversations."* Journey §1 says something
narrower and better supported: *"Two of its categories — coercive-control and the
relationship half of harassment — have no public dataset in any language."*

**Two, not three.** The plan has said since T10 that offline scripts should
evaluate *"Jigsaw (hate/harassment), PAN12 (grooming), and a threat corpus"* —
so the project already knew grooming had public data and §8 later flattened that
away. A future session reading §8 alone would hand-write grooming conversations
that a public corpus already covers. §8 is corrected in the same change as this
note.

## Per category

### grooming-exploitation — public data EXISTS

**PAN12 Sexual Predator Identification** (PAN @ CLEF 2012). Chat logs where
adults and minors converse, with predatory conversations labelled.

- Archived on Zenodo (record 3713280), a single 91.2 MB zip containing both
  training and test splits, marked **Open** access. *(Confidence: high — the
  record was fetched directly.)*
- Grooming examples come from **Perverted Justice** transcripts; non-grooming
  examples were sampled from **IRC logs and Omegle**. *(Confidence: medium —
  reported consistently across descriptions of the corpus, not verified against
  the archive itself.)*
- Later work refines it: the **eSPD** datasets (ACL 2021, *Early Detection of
  Sexual Predators in Chats*) rebuild the task around early detection, and
  **PANC** / **VTPAN** are derived variants. *(Confidence: medium.)*

Three caveats that matter more here than the availability does:

1. **The "minor" is an adult decoy.** Perverted Justice transcripts are
   volunteers posing as children. The predator's side is genuine; the child's
   side is performed. A model trained on it learns grooming *as directed at a
   decoy*, which is not obviously the same distribution as grooming on a real
   phone.
2. **The negatives invite source artifacts.** Positives from Perverted Justice
   and negatives from IRC/Omegle means a classifier can score the *source* — its
   era, platform and register — instead of the behaviour. This is the same
   failure this project has already hit twice (the leading first noise corpus in
   §3.2; the prototype/fixture coupling in #491), and it would be invisible in
   any evaluation drawn from the same two pools.
3. **The licence is not stated on the Zenodo record.** "Open access" is not a
   licence. Given the provenance — logs of adults soliciting children —
   redistribution terms and research-use conditions have to be read before a
   single file is committed or vendored. **This is a blocking question, not a
   detail.**

### coercive-control — no corpus, but a usable MODE TAXONOMY

The claim in §1 holds. The literature is explicit that there is no labelled
conversational corpus of intimate-partner abuse to train on, and that curation
would have to be done from scratch. *(Confidence: medium-high — stated directly
in IPV/NLP literature; no contrary source found.)*

The useful find is a different shape. A 2024 *Crime Science* study text-mined
**406,196 domestic and family violence police reports** and derived **48
coercive-control behaviours** — isolation, monitoring, economic control,
technology-facilitated control, threats, and so on. The underlying reports are
not public and are narratives rather than conversations, so the **data** is not
reusable. The **taxonomy** is.

That matters because of what this project learned the hard way in #492:
rewriting examples into a better *register* made scam-fraud recall worse, and
covering the *modes* the corpus had never touched is what improved it. A
generated coercive-control corpus needs a checklist of modes to cover, and
inventing one by intuition is exactly how the scam corpus ended up missing
authority-threat and verify-at-a-link. An externally derived, frequency-ordered
list of 48 behaviours is a far better starting point than a blank page.

### relationship-harassment — corpora exist, but at the wrong granularity

Also as §1 says. Sizeable labelled harassment sets exist — a ~35,000-tweet
hand-coded corpus (Golbeck et al.) and a ~24,000-tweet type-annotated corpus
with lexicons across five topics — but they are **single comments judged in
isolation**. The literature says this outright: most datasets ignore
conversational context in both annotation and modelling. *(Confidence: high —
this is a consistent, repeatedly stated limitation.)*

That is precisely the half this taxonomy cannot borrow. Relationship harassment
is defined by a pattern across messages between two known parties; a tweet
corpus cannot express it. Public harassment data remains useful for the
*content-moderation* half of the category — which is the half already covered.

## Method prior art for generating the rest

**SynBullying** (arXiv 2511.11599) is a multi-LLM *synthetic conversational*
dataset for cyberbullying — the same construction §8 proposes, published and
described. Worth reading before designing ours, both for what it does and for
how it validates that synthetic conversations are usable. *(Confidence: medium —
identified from its abstract only.)*

## What this changes

- **The corpus effort is two categories, not three.** Grooming has a public
  corpus to evaluate against first; whether it is *usable* is a licence question
  and a distribution question, not a generation question.
- **Coercive-control generation should start from the 48-behaviour taxonomy**,
  not from intuition about what coercive control sounds like.
- **Relationship-harassment is the one category with no route but generation.**

## Open questions this note does not answer

1. **PAN12's licence and redistribution terms.** Blocking. Nothing gets vendored
   or committed until this is read.
2. **Whether PAN12's distribution transfers at all** to iMessage/SMS on a phone —
   2012 IRC-era chat against present-day mobile messaging.
3. **Whether the decoy-vs-real-minor gap matters** for our use, which is triage
   over a backup rather than live intervention.
4. **Whether evaluating on PAN12 is even wanted**, given the source-artifact risk
   above. It may be that a public number on a flawed corpus is worse than no
   public number.
5. **Ethical and legal review** before any of this is fetched. These are logs of
   adults soliciting children; "it is on Zenodo" is not the end of that question.

Questions 1 and 5 are for the user, not for an agent to decide.

## Sources

- [PAN12 on Zenodo](https://zenodo.org/records/3713280)
- [PAN @ CLEF 2012 task page](https://pan.webis.de/clef12/pan12-web/sexual-predator-identification.html)
- [Early Detection of Sexual Predators in Chats (ACL 2021)](https://aclanthology.org/2021.acl-long.386.pdf)
- [eSPD datasets](https://gitlab.com/early-sexual-predator-detection/eSPD-datasets)
- [Text mining domestic violence police narratives (Crime Science, 2024)](https://link.springer.com/article/10.1186/s40163-024-00200-2)
- [Identification of IPV from free text (medRxiv)](https://www.medrxiv.org/content/10.1101/2021.12.15.21267694.full.pdf)
- [A Large Labeled Corpus for Online Harassment Research](https://www.semanticscholar.org/paper/A-Large-Labeled-Corpus-for-Online-Harassment-Golbeck-Ashktorab/3b2a962f977a081637fd683c1dc9582e12b344dd)
- [Harassment corpus + lexicon](https://github.com/Mrezvan94/Harassment-Corpus)
- [SynBullying (arXiv)](https://arxiv.org/html/2511.11599)
