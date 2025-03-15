//! Contains middleware for the [reqwest] client.

use std::str::FromStr;

use reqwest::Url;
use reqwest_middleware::RequestInitialiser;

use crate::args::Args;

/// A middleware that modifies outgoing HTTP requests from aaoffline.
pub(crate) struct AaofflineMiddleware {
    /// Whether photobucket watermarks should be automatically removed.
    fix_photobucket: bool,
    proxy: Option<String>,
}

impl From<&Args> for AaofflineMiddleware {
    fn from(args: &Args) -> Self {
        AaofflineMiddleware {
            fix_photobucket: !args.disable_photobucket_fix,
            proxy: args.proxy.clone(),
        }
    }
}

impl RequestInitialiser for AaofflineMiddleware {
    fn init(
        &self,
        mut req: reqwest_middleware::RequestBuilder,
    ) -> reqwest_middleware::RequestBuilder {
        if let Some((client, mut request)) = req
            .try_clone()
            .map(|x| x.build_split())
            .and_then(|x| x.1.map(|y| (x.0, y)).ok())
        {
            if self.fix_photobucket
                && request
                    .url()
                    .host_str()
                    .is_some_and(|x| x.contains("photobucket.com"))
            {
                req = req.header("Referer", "https://photobucket.com/");
            }
            if let Some(proxy) = self.proxy.as_ref() {
                let url = request.url_mut();
                *url =
                    Url::from_str(&format!("{proxy}{}", url.as_str())).expect("invalid proxy URL");
                req = reqwest_middleware::RequestBuilder::from_parts(client, request)
            }
        }
        req
    }
}
