function doLogout() {
  apiLogout().then((_) => {
    window.localStorage.removeItem("cratery-user");
    window.location.pathname = "/webapp/index.html";
  });
}

function onPageLoad() {
  setupFooter();
  return apiMe()
    .then((user) => {
      // setup
      document.getElementById("link-account").innerText = user.name;
      const isAdmin = user.roles.includes("admin");
      if (!isAdmin) {
        document.getElementById("link-admin").remove();
      }
      return user;
    })
    .catch(() => {
      window.localStorage.removeItem("cratery-user");
      return null;
    });
}

function setupFooter() {
  document
    .getElementById("year")
    .appendChild(document.createTextNode(new Date(Date.now()).getFullYear()));
  apiGetVersion().then((versionData) => {
    document
      .getElementById("version")
      .appendChild(
        document.createTextNode(
          `${versionData.tag.length === 0 ? "latest" : versionData.tag}, git ${
            versionData.commit
          }`
        )
      );
  });
}

function setupOnChange(inputEl, applyChange) {
  let timeoutChange = null;
  let lastKnown = inputEl.value;
  const doApply = () => {
    if (inputEl.value == lastKnown) {
      return;
    }
    applyChange(inputEl.value)
      .then(() => {
        lastKnown = inputEl.value;
        inputEl.classList.remove("dirty");
        inputEl.classList.add("wasSaved");
        setTimeout(() => {
          inputEl.classList.remove("wasSaved");
        }, 1000);
      })
      .catch(() => {
        inputEl.value = lastKnown;
        inputEl.classList.remove("dirty");
      });
  };
  const onChange = () => {
    if (inputEl.value == lastKnown) {
      return;
    }
    if (timeoutChange !== null) {
      clearTimeout(timeoutChange);
    }
    inputEl.classList.add("dirty");
    timeoutChange = setTimeout(doApply, 2000);
  };
  const handler = () => {
    // delay a little bit
    setTimeout(onChange, 50);
  };
  inputEl.addEventListener("blur", doApply);
  inputEl.addEventListener("change", handler);
  inputEl.addEventListener("keydown", handler);
}
