-- Appearance similarity for cross-camera candidates (issue #51). A secondary, ADDITIVE signal
-- alongside the existing plate+topology `score` (which still drives ranking): the visual cosine
-- similarity between the two appearances' CLIP crop embeddings, from the kernel `embeddings` store.
-- NULL when it could not be computed (no embedding task on a camera, retention pruned the vectors,
-- or the class isn't indexed) — an honest absence, distinct from a low score. Range [-1, 1].
ALTER TABLE movement_candidates ADD COLUMN appearance_score REAL;
