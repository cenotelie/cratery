async function onResponseJson(response) {
  if (response.status !== 200) {
    throw await response.json();
  } else {
    return await response.json();
  }
}

async function apiGetVersion() {
  const response = await fetch("/api/v1/version");
  return await onResponseJson(response);
}

async function apiGetRegistryInformation() {
  const response = await fetch("/api/v1/registry-information");
  return await onResponseJson(response);
}

async function apiMe() {
  const response = await fetch("/api/v1/me");
  return await onResponseJson(response);
}

async function apiOAuthLoginWithCode(code) {
  const response = await fetch("/api/v1/oauth/code", {
    method: "POST",
    body: code,
  });
  return await onResponseJson(response);
}

async function apiLogout() {
  const response = await fetch("/api/v1/logout", {
    method: "POST",
  });
  return await response.text();
}

async function apiGetUserTokens() {
  const response = await fetch("/api/v1/me/tokens");
  return await onResponseJson(response);
}

async function apiCreateUserToken(name, canWrite, canAdmin) {
  const response = await fetch(
    `/api/v1/me/tokens?canWrite=${canWrite}&canAdmin=${canAdmin}`,
    {
      method: "PUT",
      body: name,
    }
  );
  return await onResponseJson(response);
}

async function apiRevokeUserToken(token_id) {
  const response = await fetch(`/api/v1/me/tokens/${token_id}`, {
    method: "DELETE",
  });
  return await onResponseJson(response);
}

async function apiGetGlobalTokens() {
  const response = await fetch("/api/v1/admin/tokens");
  return await onResponseJson(response);
}

async function apiCreateGlobalToken(name) {
  const response = await fetch("/api/v1/admin/tokens", {
    method: "PUT",
    body: name,
  });
  return await onResponseJson(response);
}

async function apiRevokeGlobalToken(token_id) {
  const response = await fetch(`/api/v1/admin/tokens/${token_id}`, {
    method: "DELETE",
  });
  return await onResponseJson(response);
}

async function apiGetDocGenJobs() {
  const response = await fetch("/api/v1/admin/jobs/docgen");
  return await onResponseJson(response);
}

async function apiGetDocGenJobLog(jobId) {
  const response = await fetch(`/api/v1/admin/jobs/docgen/${jobId}/log`);
  return await onResponseJson(response);
}

async function apiGetWorkers() {
  const response = await fetch(`/api/v1/admin/workers`);
  return await onResponseJson(response);
}

async function apiGetUsers() {
  const response = await fetch("/api/v1/admin/users");
  return await onResponseJson(response);
}

async function apiUpdateUser(user) {
  const response = await fetch(`/api/v1/admin/users/${btoa(user.email)}`, {
    method: "PATCH",
    body: JSON.stringify(user),
    headers: [["content-type", "application/json"]],
  });
  return await onResponseJson(response);
}

async function apiDeleteUser(email) {
  const response = await fetch(`/api/v1/admin/users/${btoa(email)}`, {
    method: "DELETE",
  });
  return await onResponseJson(response);
}

async function apiDeactivateUser(email) {
  const response = await fetch(
    `/api/v1/admin/users/${btoa(email)}/deactivate`,
    {
      method: "POST",
    }
  );
  return await onResponseJson(response);
}

async function apiReactivateUser(email) {
  const response = await fetch(
    `/api/v1/admin/users/${btoa(email)}/reactivate`,
    {
      method: "POST",
    }
  );
  return await onResponseJson(response);
}

async function apiGetCratesStats() {
  const response = await fetch("/api/v1/crates/stats");
  return await onResponseJson(response);
}

async function apiGetCratesOutdatedHeads() {
  const response = await fetch("/api/v1/crates/outdated");
  return await onResponseJson(response);
}

async function apiLookupCrates(input) {
  const response = await fetch("/api/v1/crates?q=" + encodeURIComponent(input));
  const responseJson = await onResponseJson(response);
  return responseJson.crates;
}

async function apiGetCrate(crate) {
  const response = await fetch(`/api/v1/crates/${crate}`);
  return await onResponseJson(response);
}

async function apiGetCrateLastReadme(crate) {
  const response = await fetch(`/api/v1/crates/${crate}/readme`);
  if (response.status !== 200) {
    throw await response.json();
  } else {
    return await response.text();
  }
}

async function apiGetCrateReadmeAt(crate, version) {
  const response = await fetch(`/api/v1/crates/${crate}/${version}/readme`);
  if (response.status !== 200) {
    throw await response.json();
  } else {
    return await response.text();
  }
}

async function apiGetCrateOwners(crate) {
  const response = await fetch(`/api/v1/crates/${crate}/owners`);
  return await onResponseJson(response);
}

async function apiAddCrateOwner(crate, email) {
  const response = await fetch(`/api/v1/crates/${crate}/owners`, {
    method: "PUT",
    body: JSON.stringify({ users: [email] }),
    headers: [["content-type", "application/json"]],
  });
  return await onResponseJson(response);
}

async function apiRemoveCrateOwners(crate, email) {
  const response = await fetch(`/api/v1/crates/${crate}/owners`, {
    method: "DELETE",
    body: JSON.stringify({ users: [email] }),
    headers: [["content-type", "application/json"]],
  });
  return await onResponseJson(response);
}

async function apiGetCrateTargets(crate) {
  const response = await fetch(`/api/v1/crates/${crate}/targets`);
  return await onResponseJson(response);
}

async function apiSetCrateTargets(crate, targets) {
  const response = await fetch(`/api/v1/crates/${crate}/targets`, {
    method: "PATCH",
    body: JSON.stringify(targets),
    headers: [["content-type", "application/json"]],
  });
  return await onResponseJson(response);
}

async function apiGetCrateCapabilities(crate) {
  const response = await fetch(`/api/v1/crates/${crate}/capabilities`);
  return await onResponseJson(response);
}

async function apiSetCrateCapabilities(crate, capabilities) {
  const response = await fetch(`/api/v1/crates/${crate}/capabilities`, {
    method: "PATCH",
    body: JSON.stringify(capabilities),
    headers: [["content-type", "application/json"]],
  });
  return await onResponseJson(response);
}

async function apiSetCrateDeprecation(crate, isDeprecated) {
  const response = await fetch(`/api/v1/crates/${crate}/deprecated`, {
    method: "PATCH",
    body: JSON.stringify(isDeprecated),
    headers: [["content-type", "application/json"]],
  });
  return await onResponseJson(response);
}

async function apiSetCrateCanRemove(crate, canRemove) {
  const response = await fetch(`/api/v1/crates/${crate}/canremove`, {
    method: "PATCH",
    body: JSON.stringify(canRemove),
    headers: [["content-type", "application/json"]],
  });
  return await onResponseJson(response);
}

async function apiRegenCrateDoc(crate, version) {
  const response = await fetch(`/api/v1/crates/${crate}/${version}/docsregen`, {
    method: "POST",
  });
  return await onResponseJson(response);
}

async function apiRemoveCrateVersion(crate, version) {
  const response = await fetch(`/api/v1/crates/${crate}/${version}`, {
    method: "DELETE",
  });
  return await onResponseJson(response);
}

async function apiCheckCrateDeps(crate, version) {
  const response = await fetch(`/api/v1/crates/${crate}/${version}/checkdeps`, {
    method: "GET",
  });
  return await onResponseJson(response);
}

async function apiGetCrateDlStats(crate) {
  const response = await fetch(`/api/v1/crates/${crate}/dlstats`, {
    method: "GET",
  });
  return await onResponseJson(response);
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
