use crate::{
    signature::{identity::Identity, EcdsaSignature},
    CeremoniesError, Contribution, Engine, Entropy, Tau, G2,
};
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha20Rng;
use rayon::prelude::*;
use secrecy::{ExposeSecret, Secret};
use serde::{Deserialize, Serialize};
use tracing::instrument;

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct BatchContribution {
    pub contributions:   Vec<Contribution>,
    pub ecdsa_signature: EcdsaSignature,
}

impl BatchContribution {
    #[instrument(level = "info", skip_all, fields(n=self.contributions.len()))]
    pub fn receipt(&self) -> Vec<G2> {
        self.contributions.iter().map(|c| c.pot_pubkey).collect()
    }

    #[instrument(level = "info", skip_all, fields(n=self.contributions.len()))]
    pub fn add_entropy<E: Engine>(
        &mut self,
        entropy: &Entropy,
        identity: &Identity,
    ) -> Result<(), CeremoniesError> {
        let taus = derive_taus::<E>(entropy, self.contributions.len());
        let res = self
            .contributions
            .par_iter_mut()
            .zip(&taus)
            .enumerate()
            .try_for_each(|(i, (contribution, tau))| {
                contribution
                    .add_tau::<E>(tau, identity)
                    .map_err(|e| CeremoniesError::InvalidCeremony(i, e))
            });
        res
    }

    #[instrument(level = "info", skip_all, fields(n=self.contributions.len()))]
    pub fn validate<E: Engine>(&mut self) -> Result<(), CeremoniesError> {
        let res =
            self.contributions
                .par_iter_mut()
                .enumerate()
                .try_for_each(|(i, contribution)| {
                    contribution
                        .validate::<E>()
                        .map_err(|e| CeremoniesError::InvalidCeremony(i, e))
                });
        res
    }
}

fn derive_taus<E: Engine>(entropy: &Entropy, size: usize) -> Vec<Tau> {
    // TODO: ChaCha20Rng does not implement Zeroize.
    let mut rng = ChaCha20Rng::from_seed(*entropy.expose_secret());

    (0..size)
        .map(|_| {
            let entropy = Secret::new(rng.gen());
            E::generate_tau(&entropy)
        })
        .collect()
}

#[allow(clippy::missing_panics_doc)] // Does not panic.
#[must_use]
pub fn get_pot_pubkeys<E: Engine>(entropy: &Entropy) -> Vec<G2> {
    let taus = derive_taus::<E>(entropy, 4);
    let result: Vec<G2> = taus
        .into_par_iter()
        .map(|tau| {
            let mut temp = [G2::one(), G2::one()];
            E::add_tau_g2(&tau, &mut temp).unwrap();
            temp[1]
        })
        .collect();
    result
}

#[cfg(feature = "bench")]
#[doc(hidden)]
pub mod bench {
    use super::*;
    use crate::{
        bench::{rand_entropy, BATCH_SIZE},
        signature::identity::Identity,
        Arkworks, BatchTranscript, Both, BLST,
    };
    use criterion::{BatchSize, Criterion};

    pub fn group(criterion: &mut Criterion) {
        #[cfg(feature = "arkworks")]
        bench_add_tau::<Arkworks>(criterion, "arkworks");
        #[cfg(feature = "blst")]
        bench_add_tau::<BLST>(criterion, "blst");
        #[cfg(all(feature = "arkworks", feature = "blst"))]
        bench_add_tau::<Both<Arkworks, BLST>>(criterion, "both");
    }

    fn bench_add_tau<E: Engine>(criterion: &mut Criterion, name: &str) {
        // Create a non-trivial transcript
        let transcript = {
            let mut transcript = BatchTranscript::new(BATCH_SIZE.iter());
            let mut contribution = transcript.contribution();
            contribution
                .add_entropy::<E>(&rand_entropy(), &Identity::None)
                .unwrap();
            transcript
                .verify_add::<E>(contribution, Identity::None)
                .unwrap();
            transcript
        };

        criterion.bench_function(
            &format!("batch_contribution/{name}/add_tau"),
            move |bencher| {
                bencher.iter_batched(
                    || (transcript.contribution(), rand_entropy()),
                    |(mut contribution, entropy)| {
                        contribution
                            .add_entropy::<E>(&entropy, &Identity::None)
                            .unwrap();
                    },
                    BatchSize::LargeInput,
                );
            },
        );
    }
}
