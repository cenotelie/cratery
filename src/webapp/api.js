function apiGetVersion() {
  return fetch("/version").then((response) => {
    if (response.status !== 200) {
      throw response.text();
    } else {
      return response.json();
    }
  });
}

function apiGetRegistryInformation() {
  return fetch("/registry-information").then((response) => {
    if (response.status !== 200) {
      throw response.text();
    } else {
      return response.json();
    }
  });
}

function apiMe() {
  return fetch("/api/v1/me").then((response) => {
    if (response.status !== 200) {
      throw response.text();
    } else {
      return response.json();
    }
  });
}

function apiOAuthLoginWithCode(code) {
  return fetch("/api/v1/oauth/code", { method: "POST", body: code }).then(
    (response) => {
      if (response.status !== 200) {
        throw response.text();
      } else {
        return response.json();
      }
    }
  );
}

function apiLogout() {
  return fetch("/api/v1/logout", {
    method: "POST",
  }).then((r) => r.text());
}

function apiGetTokens() {
  return fetch("/api/v1/tokens").then((response) => {
    if (response.status !== 200) {
      throw response.text();
    } else {
      return response.json();
    }
  });
}

function apiCreateToken(name, canWrite, canAdmin) {
  return fetch(`/api/v1/tokens?canWrite=${canWrite}&canAdmin=${canAdmin}`, {
    method: "PUT",
    body: name,
  }).then((response) => {
    if (response.status !== 200) {
      throw response.text();
    } else {
      return response.json();
    }
  });
}

function apiRevokeToken(token_id) {
  return fetch(`/api/v1/tokens/${token_id}`, { method: "DELETE" }).then(
    (response) => {
      if (response.status !== 200) {
        throw response.text();
      } else {
        return response.text();
      }
    }
  );
}

function apiGetUsers() {
  return fetch("/api/v1/users").then((response) => {
    if (response.status !== 200) {
      throw response.text();
    } else {
      return response.json();
    }
  });
}

function apiUpdateUser(user) {
  return fetch(`/api/v1/users/${btoa(user.email)}`, {
    method: "PATCH",
    body: JSON.stringify(user),
    headers: [["content-type", "application/json"]],
  }).then((response) => {
    if (response.status !== 200) {
      throw response.text();
    } else {
      return response.json();
    }
  });
}

function apiDeleteUser(email) {
  return fetch(`/api/v1/users/${btoa(email)}`, {
    method: "DELETE",
  }).then((response) => {
    if (response.status !== 200) {
      throw response.text();
    } else {
      return response.json();
    }
  });
}

function apiDeactivateUser(email) {
  return fetch(`/api/v1/users/${btoa(email)}/deactivate`, {
    method: "POST",
  }).then((response) => {
    if (response.status !== 200) {
      throw response.text();
    } else {
      return response.text();
    }
  });
}

function apiReactivateUser(email) {
  return fetch(`/api/v1/users/${btoa(email)}/reactivate`, {
    method: "POST",
  }).then((response) => {
    if (response.status !== 200) {
      throw response.text();
    } else {
      return response.text();
    }
  });
}

function apiGetCratesStats() {
  return fetch("/api/v1/crates/stats").then((response) => {
    if (response.status !== 200) {
      throw response.text();
    } else {
      return response.json();
    }
  });
}

function apiGetCratesOutdatedHeads() {
  return fetch("/api/v1/crates/outdated").then((response) => {
    if (response.status !== 200) {
      throw response.text();
    } else {
      return response.json();
    }
  });
}

function apiLookupCrates(input) {
  return fetch("/api/v1/crates?q=" + encodeURIComponent(input))
    .then((response) => {
      if (response.status !== 200) {
        throw response.text();
      } else {
        return response.json();
      }
    })
    .then((response) => response.crates);
}

function apiGetCrate(crate) {
  return fetch(`/api/v1/crates/${crate}`).then((response) => {
    if (response.status !== 200) {
      throw response.text();
    } else {
      return response.json();
    }
  });
}

function apiGetCrateLastReadme(crate) {
  return fetch(`/api/v1/crates/${crate}/readme`).then((response) => {
    if (response.status !== 200) {
      throw response.text();
    } else {
      return response.text();
    }
  });
}

function apiGetCrateReadmeAt(crate, version) {
  return fetch(`/api/v1/crates/${crate}/${version}/readme`).then((response) => {
    if (response.status !== 200) {
      throw response.text();
    } else {
      return response.text();
    }
  });
}

function apiGetCrateOwners(crate) {
  return fetch(`/api/v1/crates/${crate}/owners`).then((response) => {
    if (response.status !== 200) {
      throw response.text();
    } else {
      return response.json();
    }
  });
}

function apiAddCrateOwner(crate, email) {
  return fetch(`/api/v1/crates/${crate}/owners`, {
    method: "PUT",
    body: JSON.stringify({ users: [email] }),
    headers: [["content-type", "application/json"]],
  }).then((response) => {
    if (response.status !== 200) {
      throw response.text();
    } else {
      return response.json();
    }
  });
}

function apiRemoveCrateOwners(crate, email) {
  return fetch(`/api/v1/crates/${crate}/owners`, {
    method: "DELETE",
    body: JSON.stringify({ users: [email] }),
    headers: [["content-type", "application/json"]],
  }).then((response) => {
    if (response.status !== 200) {
      throw response.text();
    } else {
      return response.json();
    }
  });
}

function apiGetCrateTargets(crate) {
  return fetch(`/api/v1/crates/${crate}/targets`).then((response) => {
    if (response.status !== 200) {
      throw response.text();
    } else {
      return response.json();
    }
  });
}

function apiSetCrateTargets(crate, targets) {
  return fetch(`/api/v1/crates/${crate}/targets`, {
    method: "PATCH",
    body: JSON.stringify(targets),
    headers: [["content-type", "application/json"]],
  }).then((response) => {
    if (response.status !== 200) {
      throw response.text();
    } else {
      return response.json();
    }
  });
}

function apiRegenCrateDoc(crate, version) {
  return fetch(`/api/v1/crates/${crate}/${version}/docsregen`, {
    method: "POST",
  }).then((response) => {
    if (response.status !== 200) {
      throw response.text();
    } else {
      return response.json();
    }
  });
}

function apiCheckCrateDeps(crate, version) {
  return fetch(`/api/v1/crates/${crate}/${version}/checkdeps`, {
    method: "GET",
  }).then((response) => {
    if (response.status !== 200) {
      throw response.text();
    } else {
      return response.json();
    }
  });
}

function apiGetCrateDlStats(crate) {
  return fetch(`/api/v1/crates/${crate}/dlstats`, {
    method: "GET",
  }).then((response) => {
    if (response.status !== 200) {
      throw response.text();
    } else {
      return response.json();
    }
  });
}

function getQueryParameters(queryString) {
  const regex = new RegExp("[\\?&]([a-zA-Z0-9_-]+)=([^&#]*)", "g");
  let match = null;
  let result = {};
  do {
    match = regex.exec(queryString);
    if (match !== null) {
      let name = match[1];
      let value = decodeURIComponent(match[2].replace(/\+/g, " "));
      result[name] = value;
    }
  } while (match !== null);
  return result;
}

function getDatePart(input, regexp) {
  const result = input.match(new RegExp(regexp, "g"));
  if (result === null) {
    return "";
  }
  if (result.length === 0) {
    return "";
  }
  return result.pop();
}

function toDate(date) {
  if (date instanceof Date) {
    return date;
  }
  let datePart = getDatePart(date, "[0-9]{4}-[0-9]{2}-[0-9]{2}");
  if (datePart.length === 0) {
    datePart = getDatePart(date, "[0-9]{4}/[0-9]{2}/[0-9]{2}").replace(
      "/",
      "-"
    );
  }
  let timePart = getDatePart(date, "[0-9]{2}:[0-9]{2}:[0-9]{2}");
  if (timePart.length === 0) {
    timePart = "00:00:00";
  }
  return new Date(`${datePart}T${timePart}Z`);
}

function serializeDateTime(date) {
  if (date === null) {
    return "";
  }
  date = toDate(date);
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(
    date.getDate()
  )} ${pad(date.getHours())}:${pad(date.getMinutes())}:${pad(
    date.getSeconds()
  )}`;
}

function serializeDate(date) {
  if (date === null) {
    return "";
  }
  date = toDate(date);
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(
    date.getDate()
  )}`;
}

function pad(x) {
  if (x < 10) {
    return "0" + x;
  }
  return x.toString();
}
