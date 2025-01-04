use reqwest_middleware::RequestInitialiser;

use crate::Args;

/// A middleware that modifies outgoing HTTP requests from aaoffline.
pub(crate) struct AaofflineMiddleware {
    /// Whether photobucket watermarks should be automatically removed.
    fix_photobucket: bool,
}

impl From<&Args> for AaofflineMiddleware {
    fn from(args: &Args) -> Self {
        AaofflineMiddleware {
            fix_photobucket: !args.disable_photobucket_fix,
        }
    }
}

impl RequestInitialiser for AaofflineMiddleware {
    fn init(
        &self,
        mut req: reqwest_middleware::RequestBuilder,
    ) -> reqwest_middleware::RequestBuilder {
        if let Some(request) = req.try_clone().and_then(|x| x.build().ok()) {
            if self.fix_photobucket
                && request
                    .url()
                    .host_str()
                    .is_some_and(|x| x.contains("photobucket.com"))
            {
                req = req.header("Referer", "https://photobucket.com/");
            }
        }
        req
    }
}
