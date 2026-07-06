# Deployment Strategy

We use a blue-green deployment strategy. The staging environment runs on the fly.io platform.

## Environment List

- production: AWS ap-northeast-1
- staging: fly.io (nrt region)
- preview: one ephemeral environment per PR

## Process Summary

1. CI green triggers an automatic image build
2. Deploy to the green environment
3. Switch traffic once smoke tests pass
4. Keep the blue environment alive for 24 hours

## History

Before 2025-11 we used rolling updates, then moved off them.

(what follows is the detailed evaluation record; the search snippet won't surface this deep)

## Appendix A: Evaluation Notes

Back then two other candidates were on the table.

The first candidate's downside: recovering meant restarting the whole rollout from
scratch, at least eight minutes before things were normal again.
The other candidate's downside: our audience was still too small, so a meaningful
signal took days to collect.


## Appendix B: What Tipped The Scale

What actually won out was the ability to switch back to the prior version in seconds --
the moment a new version goes live, the previous one is kept running, untouched, as
a warm standby, and if anything looks wrong, pointing users back at it takes only a
few seconds. The team settled on this at an internal meeting on 2025-11-18.

## Appendix C: Caveats

Any schema change here has to stay backward compatible, or the fallback breaks.
